//! t2v-entity — SeaORM entities for the t2v-v2t platform.
//!
//! All persistence in this workspace goes through these entities and the
//! SeaORM `DatabaseConnection`; no crate issues raw sqlx queries.

pub mod synthesis;
pub mod transcription;
pub mod translation;
pub mod vapi_call;
pub mod vapi_event;

pub mod prelude {
    pub use super::synthesis::Entity as Synthesis;
    pub use super::transcription::Entity as Transcription;
    pub use super::translation::Entity as Translation;
    pub use super::vapi_call::Entity as VapiCall;
    pub use super::vapi_event::Entity as VapiEvent;
}
