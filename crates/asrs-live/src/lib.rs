//! Live data sources: the shared base (`LiveDataSource.ts`, `LiveDataTuner.ts`), the timer queue
//! that stands in for the renderer's `setTimeout`/`setInterval`, and the tokio runtime and event
//! channel that carry socket traffic to the foreground thread.

pub mod channel;
pub mod ftc;
pub mod http;
pub mod live_data_source;
pub mod live_data_tuner;
pub mod net_options;
pub mod nt4;
pub mod phoenix;
pub mod rlog;
pub mod runtime;
mod thrown;
pub mod timers;

pub use channel::{EventReceiver, EventSender, Notify, event_channel, event_channel_notifying};
pub use live_data_source::{
    DISCARD_PERIOD_MS, LiveContext, LiveDataSource, LiveDataSourceBase, LiveDataSourceStatus,
    LiveLog, LivePreferences, OutputCallback, StatusCallback, TimeSupplier,
};
pub use live_data_tuner::{LiveDataTuner, TunedValue};
pub use net_options::{Dial, NetOptions};
pub use runtime::{LiveRuntime, LiveTask};
pub use thrown::Thrown;
pub use timers::{ManualClock, MonotonicClock, TimerId, TimerQueue};
