use mxrs_ir::{ExportLevel, OnOverlap, ScheduleUnit, ScheduledEventDecl, ScheduledEventSchedule};

/// Builder for one editable `ScheduledEvents$ScheduledEvent` document.
///
/// A scheduled event is meaningless without a microflow to run, so the
/// microflow and the cadence are constructor arguments rather than optional
/// setters — there is no valid half-built state to express.
pub struct ScheduledEventBuilder {
    decl: ScheduledEventDecl,
}

impl ScheduledEventBuilder {
    pub(crate) fn new(
        name: impl Into<String>,
        microflow: impl Into<String>,
        unit: ScheduleUnit,
    ) -> Self {
        Self {
            decl: ScheduledEventDecl::new(name, microflow, unit),
        }
    }

    pub(crate) fn into_decl(self) -> ScheduledEventDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    /// Multiplier for the unit passed to [`crate::ModuleBuilder::scheduled_event`].
    /// The writer rejects negative legacy intervals. For minute/hour modern
    /// schedules, this convenience method updates the multiplier too.
    pub fn every(&mut self, interval: i64) -> &mut Self {
        self.decl.interval = interval;
        match &mut self.decl.schedule {
            ScheduledEventSchedule::Minute { multiplier }
            | ScheduledEventSchedule::Hour { multiplier, .. } => *multiplier = interval,
            _ => {}
        }
        self
    }

    pub fn start_at(&mut self, instant: impl Into<String>) -> &mut Self {
        self.decl.start_at = instant.into();
        self
    }

    pub fn time_zone(&mut self, time_zone: impl Into<String>) -> &mut Self {
        self.decl.time_zone = time_zone.into();
        self
    }

    pub fn on_overlap(&mut self, on_overlap: OnOverlap) -> &mut Self {
        self.decl.on_overlap = on_overlap;
        self
    }

    pub fn enabled(&mut self, enabled: bool) -> &mut Self {
        self.decl.enabled = enabled;
        self
    }

    pub fn excluded(&mut self, excluded: bool) -> &mut Self {
        self.decl.excluded = excluded;
        self
    }

    pub fn export_level(&mut self, level: ExportLevel) -> &mut Self {
        self.decl.export_level = level;
        self
    }

    pub fn schedule(&mut self, schedule: ScheduledEventSchedule) -> &mut Self {
        self.decl.schedule = schedule;
        self
    }
}
