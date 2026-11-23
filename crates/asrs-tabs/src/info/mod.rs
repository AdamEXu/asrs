//! The pages of the windows without a hub renderer behind them, as headless models:
//! `src/licenses.ts`, `src/sourceListHelp.ts` and `src/betaWelcome.ts`.

pub mod beta_welcome;
pub mod licenses;
pub mod source_list_help;

pub use beta_welcome::BetaWelcome;
pub use licenses::License;
pub use source_list_help::SourceListHelp;
