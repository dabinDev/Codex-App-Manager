import type { TFn, TKey } from "../../i18n";

const ERROR_KEYS: Record<string, TKey> = {
  desktop_required: "config.error.desktopRequired",
  orange_invalid_credentials: "config.error.invalidCredentials",
  orange_forbidden: "config.error.forbidden",
  orange_rate_limited: "config.error.rateLimited",
  orange_timeout: "config.error.timeout",
  orange_network: "config.error.network",
  orange_invalid_response: "config.error.invalidResponse",
  orange_api_rejected: "config.error.apiRejected",
  orange_2fa_unsupported: "config.error.twoFactorUnsupported",
  orange_turnstile_unsupported: "config.error.turnstileUnsupported",
  orange_credential_store: "config.error.credentialStore",
  orange_signed_out: "config.error.signedOut",
  orange_key_unavailable: "config.error.keyUnavailable",
  orange_persistence: "config.error.persistence",
  unsupported_platform: "config.error.unsupportedPlatform",
  provider_path_unavailable: "config.error.providerPathUnavailable",
  provider_unsafe_destination: "config.error.providerUnsafeDestination",
  provider_busy: "config.error.providerBusy",
  provider_empty_key: "config.error.providerEmptyKey",
  provider_backup_failed: "config.error.providerIo",
  provider_io: "config.error.providerIo",
  ccswitch_unavailable: "config.error.ccsUnavailable",
  provider_models_empty: "config.error.providerModelsEmpty",
  provider_model_invalid: "config.error.providerModelInvalid",
  provider_model_duplicate: "config.error.providerModelDuplicate",
  provider_default_not_selected: "config.error.providerDefaultNotSelected",
  provider_model_unavailable: "config.error.providerModelUnavailable",
  opencodex_runtime_unavailable: "config.error.opencodexRuntimeUnavailable",
  opencodex_config_io: "config.error.opencodexConfigIo",
  opencodex_spawn_failed: "config.error.opencodexSpawnFailed",
  opencodex_not_ready: "config.error.opencodexNotReady",
  opencodex_request_failed: "config.error.opencodexRequestFailed",
  opencodex_models_invalid: "config.error.opencodexModelsInvalid",
  opencodex_models_empty: "config.error.opencodexModelsEmpty",
  opencodex_model_invalid: "config.error.opencodexModelInvalid",
  opencodex_models_too_many: "config.error.opencodexModelsTooMany",
  traecode_not_installed: "config.error.traecodeNotInstalled",
  traecode_restart_required: "config.error.traecodeRestartRequired",
  traecode_stop_failed: "config.error.traecodeStopFailed",
  traecode_launch_failed: "config.error.traecodeLaunchFailed",
  traecode_cdp_unavailable: "config.error.traecodeCdpUnavailable",
  traecode_cdp_timeout: "config.error.traecodeCdpTimeout",
  traecode_cdp_protocol: "config.error.traecodeCdpProtocol",
  traecode_connectivity_rejected: "config.error.traecodeConnectivityRejected",
  traecode_delete_failed: "config.error.traecodeDeleteFailed",
  traecode_add_failed: "config.error.traecodeAddFailed",
  traecode_refresh_failed: "config.error.traecodeRefreshFailed",
  traecode_service_unavailable: "config.error.traecodeServiceUnavailable",
  traecode_busy: "config.error.traecodeBusy",
};

export function apiConfigErrorText(code: string | null, t: TFn): string | null {
  if (!code) return null;
  return t(ERROR_KEYS[code] ?? "config.error.unknown");
}

export function apiConfigWarningText(code: string | null, t: TFn): string | null {
  if (!code) return null;
  if (code === "orange_credential_store" || code === "orange_refresh_unavailable") {
    return t("config.rememberWarning");
  }
  return apiConfigErrorText(code, t);
}
