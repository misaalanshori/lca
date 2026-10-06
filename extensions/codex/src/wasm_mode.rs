//! WASM delivery mode: the shared subscription guest, parameterized
//! by this provider's spec (gh #63 review — the dispatch lives in the
//! kit once, not once per provider).

lca_subscription::subscription_wasm_dispatcher! {
    struct CodexWasm,
    spec: crate::SPEC,
    responses_path: crate::RESPONSES_PATH,
    headers: |account_id: &str, _model: &str, token: &str| crate::headers(account_id, token),
    stored: crate::stored,
    endpoint: crate::endpoint,
    login: crate::run_login,
    logout: crate::run_logout,
    usage: crate::run_usage,
    list_models: crate::list_models,
    account_error: "no Codex account stored; run /login codex"
}
