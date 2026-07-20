//! Use cases: one doer per file (`UserCreator`, `ApiKeyIssuer`, ...), each
//! exposing a single public `execute`.

pub mod api_key_issuer;
pub mod api_key_lister;
pub mod api_key_revoker;
pub mod background_user_resolver;
pub mod default_user_resolver;
pub mod key_authenticator;
pub mod user_creator;
pub mod verified_key_cache;

#[cfg(test)]
pub mod test_doubles;
