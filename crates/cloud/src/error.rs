//! Errors worded for the user: each says what to fix.

/// The account-scoped token permissions provisioning needs (docs/cloud.md §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    WorkersScripts,
    Containers,
    R2Storage,
    AccountSettings,
}

impl Permission {
    pub const ALL: [Permission; 4] = [
        Permission::WorkersScripts,
        Permission::Containers,
        Permission::R2Storage,
        Permission::AccountSettings,
    ];

    /// The permission as the Cloudflare token editor names it.
    pub fn label(self) -> &'static str {
        match self {
            Permission::WorkersScripts => "Workers Scripts: Edit",
            Permission::Containers => "Containers: Edit",
            Permission::R2Storage => "Workers R2 Storage: Edit",
            Permission::AccountSettings => "Account Settings: Read",
        }
    }
}

pub const TOKENS_URL: &str = "https://dash.cloudflare.com/profile/api-tokens";
pub const PLANS_URL: &str = "https://dash.cloudflare.com/?to=/:account/workers/plans";

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error(
        "Cloudflare didn't accept this API token. Check that it is active, or create a new one at {TOKENS_URL}."
    )]
    InvalidToken,
    #[error("The Cloudflare API token is missing the “{}” permission. Edit the token at {TOKENS_URL} and add it.", .0.label())]
    MissingPermission(Permission),
    #[error(
        "This API token can't see any Cloudflare account. Give it access to the account Zeron should use."
    )]
    NoAccount,
    #[error(
        "This API token can see {0} Cloudflare accounts. Restrict it to the one account Zeron should use."
    )]
    MultipleAccounts(usize),
    #[error(
        "Cloudflare Containers need the Workers Paid plan. Subscribe at {PLANS_URL}, then try again."
    )]
    NotOnWorkersPaid,
    #[error(
        "Cloudflare Containers aren't enabled on this account ({0}). Open Workers & Pages → Containers in the Cloudflare dashboard to enable them, then try again."
    )]
    ContainersNotEnabled(String),
    #[error(
        "R2 isn't enabled on this Cloudflare account. Open R2 in the Cloudflare dashboard to enable it (the free tier is enough), then try again."
    )]
    R2NotEnabled,
    #[error("Connect your Cloudflare account first.")]
    NotConnected,
    #[error("Cloudflare API error on {path} ({status}): {message}")]
    Api {
        path: String,
        status: u16,
        code: Option<i64>,
        message: String,
    },
    #[error(
        "The cloud box refused the request: its key doesn't match. Remove the box and create it again."
    )]
    BoxUnauthorized,
    #[error("The cloud box isn't set up in Cloudflare anymore. Remove it and create it again.")]
    BoxUnknown,
    #[error("Couldn't reach {0}: {1}")]
    Network(String, String),
    #[error("{0}")]
    Other(String),
}

impl CloudError {
    /// The Cloudflare error code, for API errors.
    pub fn code(&self) -> Option<i64> {
        match self {
            CloudError::Api { code, .. } => *code,
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            CloudError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}
