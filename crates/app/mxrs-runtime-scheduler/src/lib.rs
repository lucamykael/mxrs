//! Scheduled-event runner for the MXRS runtime.
//!
//! Ports `lib/mxrb/runtime/scheduler.rb` (schedule normalization, due-slot
//! computation, overlap bookkeeping) and the scheduler half of
//! `shared_store.rb` (claim/renew/complete leases, process-local
//! coordinator). Execution and time are injectable: [`Scheduler::tick`]
//! takes the clock instant and an executor closure, so applications connect
//! it to the flow interpreter and tests never wait on wall-clock time.
//!
//! Deliberate divergences from the oracle: dispatch is synchronous (mxrb's
//! worker threads and lease heartbeats exist for its multi-process Ruby
//! server) — so [`Scheduler::tick`] replays the minute boundaries it slept
//! through rather than letting a long-running flow swallow another event's
//! exact `hh:mm` slot; and the `local` time zone is
//! treated as UTC — a server schedule that depends on the host's zone is
//! not reproducible, and mxrb itself degrades IANA zones to an explicit
//! error without tzinfo, which this port mirrors for every non-UTC,
//! non-numeric zone.

use std::collections::{BTreeMap, BTreeSet};

use mxrs_bson::{Bson, Document};
use mxrs_model::Module;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SchedulerError {
    #[error("{0}")]
    Invalid(String),
}

fn invalid(message: impl Into<String>) -> SchedulerError {
    SchedulerError::Invalid(message.into())
}

/// One day of missed minute boundaries is replayed after an outage; older
/// slots are dropped rather than dispatched in a burst.
const CATCH_UP_MINUTES: i64 = 1_440;

#[derive(Debug, Clone, PartialEq)]
pub enum Schedule {
    Minute {
        every: i64,
    },
    Hour {
        every: i64,
        minute: i64,
    },
    Day {
        every: i64,
        hour: i64,
        minute: i64,
    },
    Week {
        days: Vec<i64>,
        hour: i64,
        minute: i64,
    },
}

impl Schedule {
    fn kind(&self) -> &'static str {
        match self {
            Schedule::Minute { .. } => "minute",
            Schedule::Hour { .. } => "hour",
            Schedule::Day { .. } => "day",
            Schedule::Week { .. } => "week",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledJob {
    pub name: String,
    pub qualified_name: String,
    pub microflow: String,
    /// `None` when the event is disabled.
    pub schedule: Option<Schedule>,
    pub overlap: String,
    pub enabled: bool,
    /// Epoch seconds.
    pub start_at: Option<f64>,
    pub time_zone: String,
}

/// mxrb's `SchedulerCoordinator` protocol: slot claims with expiring leases
/// so multiple runners never fire the same slot twice.
pub trait Coordinator {
    #[allow(clippy::too_many_arguments)]
    fn claim(
        &mut self,
        event: &str,
        slot: &str,
        owner: &str,
        now: f64,
        lease_until: f64,
        skip_overlap: bool,
    ) -> bool;
    fn complete(&mut self, event: &str, slot: &str, owner: &str, now: f64);
    fn renew(&mut self, event: &str, slot: &str, owner: &str, lease_until: f64) -> bool;
}

#[derive(Debug, Default)]
struct Claim {
    owner: String,
    lease_expires_at: f64,
    completed_at: Option<f64>,
}

#[derive(Debug)]
struct Lease {
    slot: String,
    owner: String,
    expires_at: f64,
}

/// Port of `MemorySharedStore`'s scheduler half.
#[derive(Debug, Default)]
pub struct MemoryCoordinator {
    claims: BTreeMap<(String, String), Claim>,
    leases: BTreeMap<String, Lease>,
}

impl Coordinator for MemoryCoordinator {
    fn claim(
        &mut self,
        event: &str,
        slot: &str,
        owner: &str,
        now: f64,
        lease_until: f64,
        skip_overlap: bool,
    ) -> bool {
        let key = (event.to_string(), slot.to_string());
        if let Some(claim) = self.claims.get(&key)
            && (claim.completed_at.is_some() || claim.lease_expires_at > now)
        {
            return false;
        }
        if skip_overlap
            && let Some(lease) = self.leases.get(event)
            && lease.expires_at > now
        {
            return false;
        }
        self.claims.insert(
            key,
            Claim {
                owner: owner.to_string(),
                lease_expires_at: lease_until,
                completed_at: None,
            },
        );
        if skip_overlap {
            self.leases.insert(
                event.to_string(),
                Lease {
                    slot: slot.to_string(),
                    owner: owner.to_string(),
                    expires_at: lease_until,
                },
            );
        }
        true
    }

    fn complete(&mut self, event: &str, slot: &str, owner: &str, now: f64) {
        let key = (event.to_string(), slot.to_string());
        if let Some(claim) = self.claims.get_mut(&key)
            && claim.owner == owner
        {
            claim.completed_at = Some(now);
        }
        if let Some(lease) = self.leases.get(event)
            && lease.owner == owner
            && lease.slot == slot
        {
            self.leases.remove(event);
        }
    }

    fn renew(&mut self, event: &str, slot: &str, owner: &str, lease_until: f64) -> bool {
        let key = (event.to_string(), slot.to_string());
        let Some(claim) = self.claims.get_mut(&key) else {
            return false;
        };
        if claim.owner != owner || claim.completed_at.is_some() {
            return false;
        }
        claim.lease_expires_at = lease_until;
        if let Some(lease) = self.leases.get_mut(event)
            && lease.owner == owner
            && lease.slot == slot
        {
            lease.expires_at = lease_until;
        }
        true
    }
}

/// Loads every `ScheduledEvents$ScheduledEvent` document of every named
/// module, normalized. A malformed enabled event is a load-time error, like
/// mxrb's constructor.
pub fn jobs_from_modules(modules: &[Module]) -> Result<Vec<ScheduledJob>, SchedulerError> {
    let mut jobs = Vec::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for unit in &module.artifact_units {
            if unit.get_str("$Type").ok() != Some("ScheduledEvents$ScheduledEvent") {
                continue;
            }
            jobs.push(normalize_job(module_name, unit)?);
        }
    }
    Ok(jobs)
}

fn normalize_job(module_name: &str, source: &Document) -> Result<ScheduledJob, SchedulerError> {
    let name = source.get_str("Name").unwrap_or_default().to_string();
    let qualified = qualify(
        module_name,
        source.get_str("QualifiedName").unwrap_or(&name),
    );
    let microflow = qualify(module_name, source.get_str("Microflow").unwrap_or_default());
    let enabled = source.get_bool("Enabled").unwrap_or(true);
    let schedule = if enabled {
        Some(normalize_schedule(source)?)
    } else {
        None
    };
    Ok(ScheduledJob {
        name,
        qualified_name: qualified,
        microflow,
        schedule,
        // mxrb: `fetch(source, 'OnOverlap', …) || 'SkipNext'`. Ruby's `||`
        // only replaces nil/false, so an explicitly empty `OnOverlap` stays
        // empty there (it matches neither "skip" nor "delay") rather than
        // becoming SkipNext.
        overlap: source
            .get_str("OnOverlap")
            .unwrap_or("SkipNext")
            .to_string(),
        enabled,
        start_at: parse_time(source.get_str("StartDateTime").ok()),
        time_zone: source.get_str("TimeZone").unwrap_or_default().to_string(),
    })
}

fn normalize_schedule(source: &Document) -> Result<Schedule, SchedulerError> {
    if !source.contains_key("Schedule") {
        return legacy_schedule(source);
    }
    let schedule = source.get_document("Schedule").ok();
    let kind = schedule
        .and_then(|schedule| schedule.get_str("$Type").ok())
        .unwrap_or_default();
    let value = |keys: &[&str]| -> Result<Option<i64>, SchedulerError> {
        match schedule {
            Some(schedule) => schedule_value(schedule, keys),
            None => Ok(None),
        }
    };
    let bounded = |keys: &[&str], low: i64, high: i64| -> Result<i64, SchedulerError> {
        let number = value(keys)?.unwrap_or(0);
        if (low..=high).contains(&number) {
            Ok(number)
        } else {
            Err(invalid(format!(
                "scheduled-event {} must be in {low}..{high}",
                keys[0]
            )))
        }
    };
    if kind.ends_with("MinuteSchedule") {
        Ok(Schedule::Minute {
            every: positive(value(&["Multiplier"])?)?,
        })
    } else if kind.ends_with("HourSchedule") {
        Ok(Schedule::Hour {
            every: positive(value(&["Multiplier"])?)?,
            minute: bounded(&["MinuteOffset"], 0, 59)?,
        })
    } else if kind.ends_with("DaySchedule") {
        Ok(Schedule::Day {
            every: 1,
            hour: bounded(&["HourOfDay"], 0, 23)?,
            minute: bounded(&["MinuteOfHour"], 0, 59)?,
        })
    } else if kind.ends_with("WeekSchedule") {
        let schedule = schedule.expect("week kind implies a schedule document");
        let days: Vec<i64> = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ]
        .iter()
        .enumerate()
        .filter_map(|(number, day)| {
            matches!(schedule_flag(schedule, day), Some(true)).then_some(number as i64)
        })
        .collect();
        if days.is_empty() {
            return Err(invalid("scheduled-event week schedule requires a day"));
        }
        Ok(Schedule::Week {
            days,
            hour: bounded(&["HourOfDay"], 0, 23)?,
            minute: bounded(&["MinuteOfHour"], 0, 59)?,
        })
    } else {
        Err(invalid(format!(
            "unsupported scheduled-event schedule type {kind:?}"
        )))
    }
}

fn legacy_schedule(source: &Document) -> Result<Schedule, SchedulerError> {
    let kind = source
        .get_str("IntervalType")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let every = positive(bson_integer(source.get("Interval"), "Interval")?)?;
    match kind.as_str() {
        "minute" => Ok(Schedule::Minute { every }),
        "hour" => Ok(Schedule::Hour { every, minute: 0 }),
        "day" => Ok(Schedule::Day {
            every,
            hour: 0,
            minute: 0,
        }),
        other => Err(invalid(format!(
            "unsupported scheduled-event interval {other:?}"
        ))),
    }
}

/// mxrb's `schedule_value`: the key directly, or nested under `Properties`.
fn schedule_value(schedule: &Document, keys: &[&str]) -> Result<Option<i64>, SchedulerError> {
    for key in keys {
        if let Some(value) = bson_integer(schedule.get(key), key)? {
            return Ok(Some(value));
        }
    }
    let Ok(properties) = schedule.get_document("Properties") else {
        return Ok(None);
    };
    for key in keys {
        if let Some(value) = bson_integer(properties.get(key), key)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn schedule_flag(schedule: &Document, key: &str) -> Option<bool> {
    if let Ok(value) = schedule.get_bool(key) {
        return Some(value);
    }
    schedule.get_document("Properties").ok()?.get_bool(key).ok()
}

/// Ports mxrb's `integer(value, fallback)` — `value.nil? ? fallback :
/// Integer(value)`. Ruby's `Integer()` *raises* on a number it cannot read,
/// so a malformed `"Multiplier": "60x"` refuses the boot there. Returning
/// `None` here instead would silently install the default multiplier and run
/// the event every minute, which is exactly the failure this crate's own doc
/// says cannot happen ("a malformed enabled event is a load-time error").
///
/// `Integer(Float)` truncates toward zero, so a fractional `Double` does the
/// same rather than erroring. One divergence stays documented instead of
/// ported: Ruby reads a leading-zero string as octal (`Integer("015")` is
/// 13); this parses it as decimal 15.
fn bson_integer(value: Option<&Bson>, key: &str) -> Result<Option<i64>, SchedulerError> {
    let not_an_integer =
        |shown: String| invalid(format!("scheduled-event {key} is not an integer: {shown}"));
    match value {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Int32(value)) => Ok(Some(i64::from(*value))),
        Some(Bson::Int64(value)) => Ok(Some(*value)),
        Some(Bson::Double(value)) => {
            let truncated = value.trunc();
            if truncated.is_finite() && (i64::MIN as f64..=i64::MAX as f64).contains(&truncated) {
                Ok(Some(truncated as i64))
            } else {
                Err(not_an_integer(value.to_string()))
            }
        }
        Some(Bson::String(value)) => match value.trim().parse() {
            Ok(number) => Ok(Some(number)),
            Err(_) => Err(not_an_integer(format!("{value:?}"))),
        },
        Some(other) => Err(not_an_integer(format!("{other:?}"))),
    }
}

fn positive(value: Option<i64>) -> Result<i64, SchedulerError> {
    let number = value.unwrap_or(1);
    if number > 0 {
        Ok(number)
    } else {
        Err(invalid("scheduled-event interval must be positive"))
    }
}

fn qualify(module_name: &str, raw: &str) -> String {
    if raw.is_empty() || raw.contains('.') {
        raw.to_string()
    } else {
        format!("{module_name}.{raw}")
    }
}

/// ISO-8601 `StartDateTime` → epoch seconds; anything unparsable is ignored
/// like mxrb's `rescue ArgumentError`.
fn parse_time(raw: Option<&str>) -> Option<f64> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    parse_iso8601(text)
}

/// `Time.iso8601`'s grammar, not a lenient superset of it: only `T` separates
/// the date from the time (Ruby's space-separated form is `Time.parse`, which
/// mxrb does not call), and the date must exist on the calendar — `02-30`
/// raises there, so it is refused here instead of rolling into March.
fn parse_iso8601(text: &str) -> Option<f64> {
    // yyyy-mm-ddThh:mm:ss(.fff)?(Z|±hh:mm|±hhmm)?
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if bytes[10] != b'T' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let field = text.get(range)?;
        field
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then_some(())?;
        field.parse().ok()
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    // A leap second is a legal `Time.iso8601` input; 24:00 is not.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(fraction.len());
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => zone_offset(rest)?,
    };
    Some(
        (days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
            as f64,
    )
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `±hh:mm` or `±hhmm` → seconds, matching mxrb's `/\A[+-]\d{2}:?\d{2}\z/`
/// exactly. Stripping every `:` before counting digits (the previous shape)
/// also accepted `+023:0` and `+:0230`, and an unchecked hour field accepted
/// `+99:00` — a zone Ruby refuses as out of range.
fn zone_offset(zone: &str) -> Option<i64> {
    let (sign, rest) = match zone.split_at_checked(1)? {
        ("+", rest) => (1, rest),
        ("-", rest) => (-1, rest),
        _ => return None,
    };
    let (hours, minutes) = match rest.len() {
        4 => rest.split_at(2),
        5 if rest.as_bytes()[2] == b':' => (&rest[..2], &rest[3..]),
        _ => return None,
    };
    if !hours
        .bytes()
        .chain(minutes.bytes())
        .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60))
}

/// Resolves a job's zone to a fixed offset. UTC, `local` (treated as UTC —
/// see the crate doc) and numeric offsets are supported; IANA names are an
/// explicit error, mirroring mxrb without tzinfo.
fn zone_offset_seconds(zone: &str) -> Result<i64, SchedulerError> {
    if zone.is_empty() || zone.eq_ignore_ascii_case("UTC") || zone.eq_ignore_ascii_case("local") {
        return Ok(0);
    }
    zone_offset(zone).ok_or_else(|| {
        invalid(format!(
            "IANA time zone {zone:?} is not supported; use UTC or a numeric offset"
        ))
    })
}

struct Civil {
    minute_of_hour: i64,
    hour_of_day: i64,
    weekday: i64,
    julian_day: i64,
}

fn civil(now: f64, offset: i64) -> Civil {
    let zoned = now.floor() as i64 + offset;
    let of_day = zoned.rem_euclid(86_400);
    let days = zoned.div_euclid(86_400);
    Civil {
        minute_of_hour: of_day % 3600 / 60,
        hour_of_day: of_day / 3600,
        weekday: (days + 4).rem_euclid(7),
        julian_day: days + 2_440_588,
    }
}

pub struct Scheduler<C: Coordinator = MemoryCoordinator> {
    jobs: Vec<ScheduledJob>,
    coordinator: C,
    owner: String,
    lease_ttl: f64,
    skip_overlap: bool,
    /// The newest minute boundary already evaluated, so a tick that arrives
    /// late still sees the boundaries it slept through.
    last_minute: Option<i64>,
    last_slots: BTreeMap<String, i64>,
    running: BTreeSet<String>,
    /// `(qualified_name, message)` pairs, like mxrb's `errors` accessor.
    pub errors: Vec<(String, String)>,
}

impl Scheduler<MemoryCoordinator> {
    pub fn new(jobs: Vec<ScheduledJob>) -> Self {
        Self::with_coordinator(jobs, MemoryCoordinator::default())
    }
}

impl<C: Coordinator> Scheduler<C> {
    pub fn with_coordinator(jobs: Vec<ScheduledJob>, coordinator: C) -> Self {
        Self {
            jobs,
            coordinator,
            owner: format!("{}-{}", std::process::id(), uuid::Uuid::new_v4()),
            lease_ttl: 300.0,
            skip_overlap: true,
            last_minute: None,
            last_slots: BTreeMap::new(),
            running: BTreeSet::new(),
            errors: Vec::new(),
        }
    }

    /// mxrb's `skip_overlap:` constructor keyword (default `true`). With it
    /// off, a job's own `OnOverlap` decides, which is what makes matching
    /// `"skip"`/`"delay"` observable at all.
    #[must_use]
    pub fn with_skip_overlap(mut self, skip_overlap: bool) -> Self {
        self.skip_overlap = skip_overlap;
        self
    }

    pub fn jobs(&self) -> &[ScheduledJob] {
        &self.jobs
    }

    /// Evaluates every job against each minute boundary this tick is
    /// responsible for and executes the due ones synchronously. Returns the
    /// qualified names dispatched. An executor failure lands in
    /// [`Scheduler::errors`], never aborts the tick, and always releases the
    /// job's lease — mxrb's `ensure`.
    ///
    /// Hour, day and week schedules match an exact `hh:mm`, so a tick that
    /// lands even one second into the next minute would miss the slot
    /// entirely. mxrb never notices because it dispatches on worker threads
    /// while its ticker keeps time; this port dispatches inline, so the
    /// boundaries between the previous tick and this one are replayed here
    /// instead. Each missed slot fires exactly once, which is what mxrb's
    /// workers would have produced.
    pub fn tick(
        &mut self,
        now: f64,
        executor: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<Vec<String>, SchedulerError> {
        let mut dispatched = Vec::new();
        for instant in self.pending_minutes(now) {
            for index in 0..self.jobs.len() {
                let job = self.jobs[index].clone();
                let Some(slot) = due_slot(&job, instant)? else {
                    continue;
                };
                // Leases are wall-clock bookkeeping, so they use the real
                // `now` even while replaying an earlier boundary.
                if !self.reserve(&job, slot, now) {
                    continue;
                }
                let slot_key = format!(
                    "{}:{slot}",
                    job.schedule
                        .as_ref()
                        .expect("due jobs have a schedule")
                        .kind()
                );
                if let Err(message) = executor(&job.microflow) {
                    self.errors.push((job.qualified_name.clone(), message));
                }
                self.coordinator
                    .complete(&job.qualified_name, &slot_key, &self.owner, now);
                self.running.remove(&job.qualified_name);
                dispatched.push(job.qualified_name.clone());
            }
        }
        Ok(dispatched)
    }

    /// The minute boundaries `(last_minute, floor(now / 60)]`, as epoch
    /// seconds, capped at [`CATCH_UP_MINUTES`] so a process that was down for
    /// a week does not replay the week.
    ///
    /// Boundaries are evaluated at `minute * 60`, never at `now`, which makes
    /// a replayed slot indistinguishable from a punctual one. The one visible
    /// consequence is sub-minute `StartDateTime` anchoring: a job starting at
    /// `10:00:30` is not due at the `10:00:00` boundary, so it first fires at
    /// `10:01`. The clock is also treated as monotonic — a backwards jump
    /// never re-opens boundaries that were already evaluated.
    fn pending_minutes(&mut self, now: f64) -> Vec<f64> {
        let current = (now.floor() as i64).div_euclid(60);
        let first = match self.last_minute {
            Some(last) if last < current => (last + 1).max(current - CATCH_UP_MINUTES + 1),
            _ => current,
        };
        self.last_minute = Some(self.last_minute.map_or(current, |last| last.max(current)));
        (first..=current)
            .map(|minute| (minute * 60) as f64)
            .collect()
    }

    fn reserve(&mut self, job: &ScheduledJob, slot: i64, now: f64) -> bool {
        if self.last_slots.get(&job.qualified_name) == Some(&slot) {
            return false;
        }
        let skip_overlap = self.skip_overlap(job);
        if skip_overlap && self.running.contains(&job.qualified_name) {
            return false;
        }
        let slot_key = format!(
            "{}:{slot}",
            job.schedule
                .as_ref()
                .expect("due jobs have a schedule")
                .kind()
        );
        let claimed = self.coordinator.claim(
            &job.qualified_name,
            &slot_key,
            &self.owner,
            now,
            now + self.lease_ttl,
            skip_overlap,
        );
        if !claimed {
            return false;
        }
        self.last_slots.insert(job.qualified_name.clone(), slot);
        self.running.insert(job.qualified_name.clone());
        true
    }

    fn skip_overlap(&self, job: &ScheduledJob) -> bool {
        self.skip_overlap || {
            let overlap = job.overlap.to_ascii_lowercase();
            overlap.contains("skip") || overlap.contains("delay")
        }
    }
}

/// mxrb's `due_slot`: the slot index when the job is due at `now`, `None`
/// otherwise. Minute/hour slots are epoch-based; day/week slots are the
/// zone-local Julian day.
pub fn due_slot(job: &ScheduledJob, now: f64) -> Result<Option<i64>, SchedulerError> {
    if !job.enabled {
        return Ok(None);
    }
    let Some(schedule) = &job.schedule else {
        return Ok(None);
    };
    if let Some(start) = job.start_at
        && now < start
    {
        return Ok(None);
    }
    let offset = zone_offset_seconds(&job.time_zone)?;
    let time = civil(now, offset);
    let epoch = now.floor() as i64;
    Ok(match schedule {
        Schedule::Minute { every } => {
            let minute = epoch / 60;
            let anchor = job.start_at.map_or(0, |start| start.floor() as i64 / 60);
            ((minute - anchor) % every == 0).then_some(minute)
        }
        Schedule::Hour { every, minute } => {
            if time.minute_of_hour != *minute {
                return Ok(None);
            }
            let hour = epoch / 3600;
            let anchor = job.start_at.map_or(0, |start| start.floor() as i64 / 3600);
            ((hour - anchor) % every == 0).then_some(hour)
        }
        Schedule::Day {
            every,
            hour,
            minute,
        } => {
            if time.hour_of_day != *hour || time.minute_of_hour != *minute {
                return Ok(None);
            }
            let anchor = job
                .start_at
                .map_or(0, |start| civil(start, offset).julian_day);
            ((time.julian_day - anchor) % every == 0).then_some(time.julian_day)
        }
        Schedule::Week { days, hour, minute } => {
            if !days.contains(&time.weekday)
                || time.hour_of_day != *hour
                || time.minute_of_hour != *minute
            {
                return Ok(None);
            }
            Some(time.julian_day)
        }
    })
}

#[cfg(test)]
mod tests {
    use mxrs_bson::doc;

    use super::*;

    fn event(overrides: Document) -> Document {
        let mut base = doc! {
            "$Type": "ScheduledEvents$ScheduledEvent",
            "Name": "Nightly",
            "Microflow": "RunNightly",
            "Enabled": true,
        };
        for (key, value) in overrides {
            base.insert(key, value);
        }
        base
    }

    fn job(overrides: Document) -> ScheduledJob {
        normalize_job("App", &event(overrides)).unwrap()
    }

    #[test]
    fn schedules_normalize_from_modern_legacy_and_properties_shapes() {
        let minute = job(
            doc! { "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 15 } },
        );
        assert_eq!(minute.schedule, Some(Schedule::Minute { every: 15 }));
        assert_eq!(minute.qualified_name, "App.Nightly");
        assert_eq!(minute.microflow, "App.RunNightly");

        let hour = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$HourSchedule",
            "Properties": doc! { "Multiplier": 2, "MinuteOffset": 30 },
        } });
        assert_eq!(
            hour.schedule,
            Some(Schedule::Hour {
                every: 2,
                minute: 30
            })
        );

        let day = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$DaySchedule", "HourOfDay": 3, "MinuteOfHour": 30,
        } });
        assert_eq!(
            day.schedule,
            Some(Schedule::Day {
                every: 1,
                hour: 3,
                minute: 30
            })
        );

        let week = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$WeekSchedule",
            "Tuesday": true, "Saturday": true, "HourOfDay": 6, "MinuteOfHour": 0,
        } });
        assert_eq!(
            week.schedule,
            Some(Schedule::Week {
                days: vec![2, 6],
                hour: 6,
                minute: 0
            })
        );

        let legacy = job(doc! { "IntervalType": "Hour", "Interval": 4 });
        assert_eq!(
            legacy.schedule,
            Some(Schedule::Hour {
                every: 4,
                minute: 0
            })
        );

        let disabled = job(doc! { "Enabled": false, "IntervalType": "bogus" });
        assert_eq!(disabled.schedule, None);
        assert!(!disabled.enabled);
    }

    #[test]
    fn malformed_enabled_events_fail_loudly_with_the_oracle_messages() {
        let cases = [
            (
                doc! { "Schedule": doc! { "$Type": "ScheduledEvents$WeekSchedule", "HourOfDay": 6 } },
                "scheduled-event week schedule requires a day",
            ),
            (
                doc! { "Schedule": doc! { "$Type": "ScheduledEvents$DaySchedule", "HourOfDay": 99 } },
                "scheduled-event HourOfDay must be in 0..23",
            ),
            (
                doc! { "Schedule": doc! { "$Type": "ScheduledEvents$Cron" } },
                "unsupported scheduled-event schedule type \"ScheduledEvents$Cron\"",
            ),
            (
                doc! { "IntervalType": "fortnight" },
                "unsupported scheduled-event interval \"fortnight\"",
            ),
            (
                doc! { "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 0 } },
                "scheduled-event interval must be positive",
            ),
        ];
        for (overrides, message) in cases {
            let error = normalize_job("App", &event(overrides)).unwrap_err();
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn due_slots_follow_minute_hour_day_and_week_rules_with_zones() {
        // 2000-02-29 (a Tuesday) starts at epoch 951_782_400.
        let midnight = 951_782_400.0_f64;
        let at_1400 = midnight + 14.0 * 3600.0;
        let quarter = job(
            doc! { "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 15 } },
        );
        // 13:59 is not a multiple of 15 minutes from the epoch; 14:00 is.
        assert_eq!(due_slot(&quarter, at_1400 - 60.0).unwrap(), None);
        assert!(due_slot(&quarter, at_1400).unwrap().is_some());

        let half_past = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$HourSchedule", "Multiplier": 1, "MinuteOffset": 30,
        } });
        assert_eq!(due_slot(&half_past, at_1400).unwrap(), None);
        let at_1430 = at_1400 + 1800.0;
        assert!(due_slot(&half_past, at_1430).unwrap().is_some());

        // Daily at 16:30 in +02:00 == 14:30 UTC.
        let daily = job(doc! {
            "TimeZone": "+02:00",
            "Schedule": doc! { "$Type": "ScheduledEvents$DaySchedule", "HourOfDay": 16, "MinuteOfHour": 30 },
        });
        assert!(due_slot(&daily, at_1430).unwrap().is_some());
        assert_eq!(due_slot(&daily, at_1400).unwrap(), None);

        // Weekly on Tuesday at 14:30 UTC — 2000-02-29 is a Tuesday.
        let weekly = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$WeekSchedule", "Tuesday": true,
            "HourOfDay": 14, "MinuteOfHour": 30,
        } });
        assert!(due_slot(&weekly, at_1430).unwrap().is_some());
        assert_eq!(due_slot(&weekly, at_1430 + 86_400.0).unwrap(), None);

        // A start date defers everything before it.
        let deferred = job(doc! {
            "StartDateTime": "2001-01-01T00:00:00Z",
            "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 1 },
        });
        assert_eq!(due_slot(&deferred, at_1430).unwrap(), None);

        // IANA zones fail explicitly.
        let zoned = job(doc! {
            "TimeZone": "America/Manaus",
            "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 1 },
        });
        assert!(due_slot(&zoned, at_1430).is_err());
    }

    #[test]
    fn ticks_dispatch_each_slot_once_collect_errors_and_release_leases() {
        let every_minute = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 1,
        } });
        let mut scheduler = Scheduler::new(vec![every_minute]);
        let calls = std::cell::RefCell::new(Vec::new());
        let mut executor = |flow: &str| {
            calls.borrow_mut().push(flow.to_string());
            if calls.borrow().len() == 2 {
                Err("boom".to_string())
            } else {
                Ok(())
            }
        };
        let minute_one = 60.0;
        assert_eq!(
            scheduler.tick(minute_one, &mut executor).unwrap(),
            ["App.Nightly"]
        );
        // Same slot: never twice.
        assert!(
            scheduler
                .tick(minute_one + 30.0, &mut executor)
                .unwrap()
                .is_empty()
        );
        // Next slot dispatches again; the executor error is collected.
        assert_eq!(
            scheduler.tick(minute_one + 60.0, &mut executor).unwrap(),
            ["App.Nightly"]
        );
        assert_eq!(*calls.borrow(), ["App.RunNightly", "App.RunNightly"]);
        assert_eq!(scheduler.errors.len(), 1);
        assert_eq!(scheduler.errors[0].0, "App.Nightly");
        assert_eq!(scheduler.errors[0].1, "boom");
        // The lease was completed, so the following slot still fires.
        assert_eq!(
            scheduler.tick(minute_one + 120.0, &mut executor).unwrap(),
            ["App.Nightly"]
        );
    }

    fn module_with(units: Vec<Document>) -> Module {
        Module {
            id: String::new(),
            name: Some("App".to_string()),
            sort_index: None,
            from_app_store: false,
            app_store_guid: None,
            app_store_version: None,
            export_level: String::new(),
            domain_model: None,
            pages: Vec::new(),
            microflows: Vec::new(),
            nanoflows: Vec::new(),
            rules: Vec::new(),
            menus: Vec::new(),
            module_roles: Vec::new(),
            artifact_units: units,
        }
    }

    #[test]
    fn jobs_load_from_a_module_skipping_every_other_artifact_unit() {
        let module = module_with(vec![
            doc! { "$Type": "Microflows$Microflow", "Name": "NotAnEvent" },
            event(
                doc! { "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 5 } },
            ),
            {
                let mut other = event(doc! { "IntervalType": "Day", "Interval": 2 });
                other.insert("Name", "Cleanup");
                other.insert("Microflow", "App.RunCleanup");
                other
            },
        ]);
        let jobs = jobs_from_modules(std::slice::from_ref(&module)).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].qualified_name, "App.Nightly");
        assert_eq!(jobs[0].schedule, Some(Schedule::Minute { every: 5 }));
        assert_eq!(jobs[1].qualified_name, "App.Cleanup");
        // An already-qualified microflow name is not qualified twice.
        assert_eq!(jobs[1].microflow, "App.RunCleanup");
        assert_eq!(
            jobs[1].schedule,
            Some(Schedule::Day {
                every: 2,
                hour: 0,
                minute: 0
            })
        );

        // A malformed *enabled* event refuses the whole load, like mxrb's
        // constructor — it never degrades to a default schedule.
        let broken = module_with(vec![event(doc! { "IntervalType": "century" })]);
        assert!(jobs_from_modules(std::slice::from_ref(&broken)).is_err());
    }

    #[test]
    fn malformed_schedule_numbers_are_load_errors_not_silent_defaults() {
        // Ruby's `Integer("60x")` raises; defaulting to 1 would turn a typo
        // into an event firing every single minute.
        let error = normalize_job(
            "App",
            &event(
                doc! { "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": "60x" } },
            ),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "scheduled-event Multiplier is not an integer: \"60x\""
        );
        assert!(
            normalize_job(
                "App",
                &event(doc! { "IntervalType": "Hour", "Interval": "x" })
            )
            .is_err()
        );
        // A well-formed numeric string still reads, and `Integer(Float)`
        // truncates toward zero rather than failing.
        assert_eq!(
            job(
                doc! { "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": "15" } }
            )
            .schedule,
            Some(Schedule::Minute { every: 15 })
        );
        assert_eq!(
            job(doc! { "Schedule": doc! {
                "$Type": "ScheduledEvents$HourSchedule", "Multiplier": 1, "MinuteOffset": 30.9,
            } })
            .schedule,
            Some(Schedule::Hour {
                every: 1,
                minute: 30
            })
        );
    }

    #[test]
    fn zone_offsets_and_start_dates_follow_the_oracle_grammar_exactly() {
        assert_eq!(zone_offset("+02:00"), Some(7200));
        assert_eq!(zone_offset("-0230"), Some(-9000));
        // Shapes the old `remove every colon` parse accepted by accident.
        assert_eq!(zone_offset("+023:0"), None);
        assert_eq!(zone_offset("+:0230"), None);
        assert_eq!(zone_offset("+99:00"), None);
        assert_eq!(zone_offset("0200"), None);

        assert_eq!(parse_time(Some("1970-01-01T00:00:01Z")), Some(1.0));
        assert_eq!(
            parse_time(Some("2000-02-29T00:00:00Z")),
            Some(951_782_400.0)
        );
        // `Time.iso8601` rejects all of these; `Time.parse` is not what mxrb
        // calls, so neither does this.
        assert_eq!(parse_time(Some("2001-02-29T00:00:00Z")), None);
        assert_eq!(parse_time(Some("2000-04-31T00:00:00Z")), None);
        assert_eq!(parse_time(Some("2000-01-01 00:00:00Z")), None);
        assert_eq!(parse_time(Some("2000-01-01T24:00:00Z")), None);
        assert_eq!(parse_time(Some("not a date")), None);
    }

    #[test]
    fn start_dates_anchor_the_interval_they_do_not_only_defer_it() {
        // Start 2000-02-29 00:07:00 UTC, every 15 minutes: the slots are
        // :07, :22, :37 … not :00, :15, :30.
        let start = 951_782_400.0 + 7.0 * 60.0;
        let anchored = job(doc! {
            "StartDateTime": "2000-02-29T00:07:00Z",
            "Schedule": doc! { "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 15 },
        });
        assert!(due_slot(&anchored, start).unwrap().is_some());
        assert_eq!(due_slot(&anchored, start + 60.0).unwrap(), None);
        assert!(due_slot(&anchored, start + 15.0 * 60.0).unwrap().is_some());

        // Hour schedules anchor on the hour the start date falls in.
        let hourly = job(doc! {
            "StartDateTime": "2000-02-29T01:00:00Z",
            "Schedule": doc! {
                "$Type": "ScheduledEvents$HourSchedule", "Multiplier": 3, "MinuteOffset": 0,
            },
        });
        let one_am = 951_782_400.0 + 3600.0;
        assert!(due_slot(&hourly, one_am).unwrap().is_some());
        assert_eq!(due_slot(&hourly, one_am + 3600.0).unwrap(), None);
        assert!(due_slot(&hourly, one_am + 3.0 * 3600.0).unwrap().is_some());
    }

    #[test]
    fn a_late_tick_replays_the_minute_boundaries_it_slept_through() {
        // 03:00 daily. A flow that overruns its minute must not swallow it.
        let daily = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$DaySchedule", "HourOfDay": 3, "MinuteOfHour": 0,
        } });
        let mut scheduler = Scheduler::new(vec![daily]);
        let mut nothing = |_: &str| Ok(());
        let before = 951_782_400.0 + 2.0 * 3600.0 + 59.0 * 60.0; // 02:59:00
        assert!(scheduler.tick(before, &mut nothing).unwrap().is_empty());
        // The next tick lands at 03:00:20 — inside the slot — but a tick at
        // 03:01:30 would have missed 03:00 entirely before the catch-up.
        let late = 951_782_400.0 + 3.0 * 3600.0 + 90.0;
        assert_eq!(scheduler.tick(late, &mut nothing).unwrap(), ["App.Nightly"]);
        // Replayed once, never again.
        assert!(
            scheduler
                .tick(late + 60.0, &mut nothing)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn catch_up_is_capped_and_never_walks_a_clock_backwards() {
        let every_minute = job(doc! { "Schedule": doc! {
            "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 1,
        } });
        let mut scheduler = Scheduler::new(vec![every_minute]);
        let dispatches = std::cell::RefCell::new(0usize);
        let mut count = |_: &str| {
            *dispatches.borrow_mut() += 1;
            Ok(())
        };
        scheduler.tick(60.0, &mut count).unwrap();
        // Two days later: only one day of boundaries is replayed, and a
        // job due every minute still dispatches once per tick because each
        // boundary reuses the same `last_slots` entry.
        let two_days = 60.0 + 2.0 * 86_400.0;
        assert_eq!(
            scheduler.pending_minutes(two_days).len(),
            CATCH_UP_MINUTES as usize
        );
        // A clock that jumps backwards evaluates only that one minute, and
        // the high-water mark stays put — the next forward tick does not
        // walk the whole gap a second time.
        assert_eq!(scheduler.pending_minutes(60.0), vec![60.0]);
        assert_eq!(
            scheduler.pending_minutes(two_days + 60.0),
            vec![two_days + 60.0]
        );
        assert_eq!(*dispatches.borrow(), 1);
    }

    #[test]
    fn overlap_policy_is_the_events_own_once_the_global_switch_is_off() {
        let overlap_job = |overlap: Option<&str>| {
            let mut overrides = doc! { "Schedule": doc! {
                "$Type": "ScheduledEvents$MinuteSchedule", "Multiplier": 1,
            } };
            if let Some(overlap) = overlap {
                overrides.insert("OnOverlap", overlap);
            }
            job(overrides)
        };
        // Ruby's `fetch(...) || 'SkipNext'` only replaces nil, so an
        // explicitly empty OnOverlap stays empty and matches nothing.
        assert_eq!(overlap_job(Some("")).overlap, "");
        assert_eq!(overlap_job(None).overlap, "SkipNext");

        let never = overlap_job(Some(""));
        let delayed = overlap_job(Some("DelayNext"));
        let default = overlap_job(None);
        let global = Scheduler::new(Vec::new());
        // With mxrb's default `skip_overlap: true`, every job skips.
        for job in [&never, &delayed, &default] {
            assert!(global.skip_overlap(job));
        }
        // With it off, `OnOverlap` decides — which is the only way the
        // "skip"/"delay" matching is observable at all.
        let per_job = Scheduler::new(Vec::new()).with_skip_overlap(false);
        assert!(!per_job.skip_overlap(&never));
        assert!(per_job.skip_overlap(&delayed));
        assert!(per_job.skip_overlap(&default));
    }

    #[test]
    fn coordinators_refuse_duplicate_claims_and_respect_overlap_leases() {
        let mut coordinator = MemoryCoordinator::default();
        assert!(coordinator.claim("E", "minute:1", "a", 0.0, 300.0, true));
        // Same slot, other owner: refused while leased.
        assert!(!coordinator.claim("E", "minute:1", "b", 10.0, 310.0, true));
        // Other slot with skip_overlap: refused while the event lease lives.
        assert!(!coordinator.claim("E", "minute:2", "b", 10.0, 310.0, true));
        coordinator.complete("E", "minute:1", "a", 20.0);
        // Completed slots stay claimed forever; new slots open up.
        assert!(!coordinator.claim("E", "minute:1", "b", 400.0, 700.0, true));
        assert!(coordinator.claim("E", "minute:2", "b", 400.0, 700.0, true));
        assert!(coordinator.renew("E", "minute:2", "b", 900.0));
        assert!(!coordinator.renew("E", "minute:2", "intruder", 900.0));
    }
}
