//! Usage-fetch-failure marker mirroring `src/providers/usage-fetch-failure.ts`.
//!
//! The marker distinguishes "the credential was fine but the usage fetch failed" from a
//! credential/auth failure. Only the credential-bearing providers (Claude) set it; agy never
//! does. Wired with the Claude provider port (epic #8).

use crate::types::ProviderQuota;

pub fn with_usage_fetch_failure(provider: ProviderQuota) -> ProviderQuota {
    provider
}

pub fn is_usage_fetch_failure(_provider: &ProviderQuota) -> bool {
    false
}
