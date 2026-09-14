use mxrs_ir::{OnOverlap, ScheduleUnit, ScheduledEventDecl};

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
    /// The writer rejects a non-positive interval, and any interval other than
    /// `1` for [`ScheduleUnit::Days`].
    pub fn every(&mut self, interval: i32) -> &mut Self {
        self.decl.interval = interval;
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
}
