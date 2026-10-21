use royaltracker_crypto::Cipher;
use royaltracker_storage::DefaultRepo;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub repo: Arc<DefaultRepo>,
    pub cipher: Arc<Cipher>,
    pub bot_token: Arc<String>,
    pub rcg_basic_auth_b64: Arc<String>,
    pub princess: Arc<royaltracker_api::princess::PrincessGuestClient>,
    /// Fleet-wide Princess extras, shared with the public tier (6 h TTL).
    pub princess_extras: Arc<royaltracker_core::PrincessExtrasCache>,
}
