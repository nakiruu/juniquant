# Schwab OAuth 2.0

The authentication flow. Every detail matters because the token is the
only thing standing between code and capital.

> **Phase: later.** Execution and live trading come last in the build order
> (`outline.md` "Project phase and build order"). Kept as the specification
> for that phase; do not build or refine it unless asked. Code blocks are
> sketches (not compiled, not normative).

## 1. Token Lifetimes

| Token | Lifetime | Source / label |
|---|---|---|
| Access token | ~30 minutes | COMMUNITY-REPORTED (Schwabdev `welcome.md` / `setupguide.md`; not in the OpenAPI spec) |
| Refresh token | ~7 days, fixed, no sliding extension reported | COMMUNITY-REPORTED (same source); UNVERIFIED on the login-gated developer portal. Live item: confirm the real lifetime and whether the token response carries an expiry field |

The access token is short-lived. The refresh token is the long-lived
credential. When the refresh token expires, the account owner must
re-authorize manually in a browser.

**Expiry alert (before the halt).** The adapter stores the UTC time of the
owner's last authorization next to the refresh token. It writes an audit row
`broker_auth_expiring` (`WARN`) and shows it on the dashboard at 48 h, 24 h and
6 h before the *assumed* expiry (authorization time + 7 days; the assumption
is COMMUNITY-REPORTED, so the warning is conservative if the true lifetime is
longer and late if it is shorter: the live item above fixes this). Alerts are
dashboard-only (owner decision B4, 2026-10-10; `09-lock-state-machine` §10): no
email or push channel exists.

## 2. The Flow

**Initial authorization (manual, once per refresh cycle):**

1. Build the authorization URL with the app key and redirect URI.
2. Owner visits the URL, logs in, approves the app.
3. Schwab redirects to the registered URI with an authorization code.
4. Exchange the code for an access token and a refresh token.
5. Store the refresh token in the state directory and record the authorization time.

**Token refresh (automatic, every ~30 minutes):**

1. POST to the token endpoint with the refresh token.
2. Receive a new access token and (optionally) a new refresh token.
3. Update the stored refresh token if a new one is issued (and, if a new one is
   issued, the authorization time stays that of the owner's browser login unless
   the response says otherwise: do not assume a sliding window).

**Refresh token expiry (every ~7 days):**

1. The token endpoint returns an error.
2. Raise `BrokerAuthExpired`.
3. Halt all trading (halt kind `AUTH_EXPIRED`, which a clean audit clears
   automatically after the owner has re-authorized; `09-lock-state-machine` §8b).
4. Notify the owner to re-authorize.

A REST or token failure that is not an expiry (network, 5xx, 429) is a
`could-not-verify` condition, not a breach: it denies orders (fail closed) and
makes the audit `INCOMPLETE`; it never writes `lock_violation_detected`.

## 3. Storage

The refresh token is stored at:

    $QUANT_STATE/schwab_refresh_token      (host: /mnt/cache/appdata/quant/)

Permissions: `0600` (owner read/write only). The container runs as UID
99 (Unraid `nobody`). The file is owned by 99:100.

**Do not** commit the token to Git. **Do not** log it. **Do not** store
it in a Parquet file. It is a credential, not data. Only the Rust process
holds it: the frontend and Julia never see it (constitution §4).

## 4. Error Handling

| Error | Meaning | Action |
|---|---|---|
| 401 Unauthorized | Access token expired or invalid | Refresh and retry once |
| 403 Forbidden | App not authorized for this endpoint | Log FAIL, halt |
| `BrokerAuthExpired` | Refresh token expired | Halt (`AUTH_EXPIRED`), notify owner |
| 429 Too Many Requests | Rate limit | Exponential backoff, honour `Retry-After` |

## 5. Implementation Notes

The Rust adapter uses `reqwest` for the token POST. The request is
(endpoint and form per Schwab's OAuth docs as mirrored in client libraries;
CLIENT-SOURCE, verify against the portal):

    POST https://api.schwabapi.com/v1/oauth/token
    Content-Type: application/x-www-form-urlencoded
    Authorization: Basic {base64(app_key:app_secret)}
    
    grant_type=refresh_token&refresh_token={token}

The response is JSON with `access_token`, `refresh_token`, `expires_in`.
Store `expires_in` and refresh proactively at `expires_in - 60` seconds.

## 6. What Not to Do

- Do not hardcode the app key or secret. Read from environment.
- Do not store tokens in the Parquet lake.
- Do not log token values.
- Do not assume the refresh token never expires.
- Do not retry authorization failures more than once.
- Do not treat a token or REST outage as a lock violation.
