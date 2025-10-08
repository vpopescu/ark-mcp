# Google Identity Platform Integration Guide

This guide explains how to configure Google as an OpenID Connect (OIDC) provider for Ark, obtain the necessary identifiers, and add the correct configuration. It mirrors the Entra ID setup but highlights Google-specific differences.

---
## 1. Terminology
- **Client ID**: Public identifier issued by Google.
- **Client Secret** (optional for confidential model): Secret issued for Web application types.
- **Redirect URI**: Callback endpoint in Ark (`/auth/callback`). Must match exactly.
- **Discovery**: Google provides standard OIDC discovery at `https://accounts.google.com/.well-known/openid-configuration`.

Ark uses Authorization Code + PKCE (public client defaults). A secret is optional unless you configure a confidential Web application flow.

---
## 2. Create a Google Cloud Project (if needed)
1. Visit: https://console.cloud.google.com/
2. Create or select an existing project.
3. Make sure billing (if required) is set up (not needed for simple OAuth generally).

---
## 3. Enable OAuth Consent Screen
1. In the Google Cloud Console: APIs & Services → OAuth consent screen.
2. Choose **External** (for most use cases) or **Internal** (Google Workspace domain only).
3. Fill in required app info (App name, user support email, developer contact email).
4. Add scopes (the basic OIDC scopes are added automatically during requests; you can leave defaults).
5. Save and proceed through summary.
6. For production, publish the app. For development, test mode with test users is fine.

---
## 4. Create OAuth 2.0 Credentials
1. APIs & Services → Credentials → + Create Credentials → OAuth client ID.
2. **Application type (IMPORTANT)**:
   - **Web application** (Recommended) - Supports server-side token exchange with optional client secrets
   - Avoid "Desktop" or "Single Page Application" for server-side flows as they may require different authentication patterns
3. Name: `Ark MCP Admin Console` or your preferred name.
4. **Authorized redirect URIs** (add these exact URIs):
   - `http://localhost:8000/auth/callback`  (for local HTTP development)
   - `https://localhost:8000/auth/callback` (for local HTTPS development)
   - `https://127.0.0.1:8000/auth/callback` (alternative for local HTTPS)
   - (For production) `https://your-domain.example.com/auth/callback`
5. Click **Create**.
6. Copy the **Client ID** and **Client Secret** (if Web application was chosen).

**Note**: Web applications support both PKCE (for security) and client secrets (for additional authentication). This provides the most flexibility for server-side OAuth flows.

---
## 5. Scopes
Ark defaults to:
```
openid profile email
```
You can append `https://www.googleapis.com/auth/userinfo.email` etc., but the basic OIDC claims are usually enough. Avoid over-scoping early.

---
## 6. Authority & Discovery
Use the stable Google issuer / authority:
```
https://accounts.google.com
```
Discovery automatically resolves:
- `authorization_endpoint`
- `token_endpoint`
- `jwks_uri`

No tenant segment is required (multi-tenant globally).

---
## 7. Ark Configuration Snippet
```yaml
auth:
  enabled: true
  provider: google
  session:
    timeout_seconds: 3600
    cookie_name: ark_session
    cookie_secure: true         # Set to true for HTTPS (recommended)
    cookie_http_only: true
    same_site: Lax
  providers:
    - name: google
      authority: "https://accounts.google.com"
      client_id: "<GOOGLE_CLIENT_ID>"
      discovery: true
      scopes: "openid profile email"
      # client_secret: "<CLIENT_SECRET>"  # Add this if using Web application type
```

**Notes:**
- For local development with HTTPS, set `cookie_secure: true`
- For local development with HTTP only, set `cookie_secure: false`
- The `client_secret` is recommended for production Web applications
- Google's server-side OAuth flow is more secure than client-side flows

---
## 8. Frontend Flow Recap
1. UI calls `/auth/login` → JSON `{ redirect: "https://accounts.google.com/o/oauth2/v2/auth?..." }` or `/auth/login?mode=redirect` for 302.
2. Browser navigates to Google sign-in.
3. User authenticates → Google redirects to `/auth/callback` with `code` + `state`.
4. Ark exchanges `code` at Google `token_endpoint` and validates ID token.
5. Session cookie set; frontend re-polls `/auth/status` and shows user name/email (claims: `sub`, `email`, `name`).

---
## 9. Testing Locally
1. Start Ark on `https://localhost:8000` (with TLS enabled)
2. Access the login at `https://127.0.0.1:8000/auth/login` or `https://localhost:8000/auth/login`
3. Check that redirect URI in the login request uses HTTPS
4. Ensure the redirect URI in Google Console exactly matches what Ark sends
5. Verify `/auth/login` response includes the correct `redirect_uri` parameter
6. After Google authentication, check that `/auth/callback` completes successfully

**Important**: The redirect URI must match exactly between:
- What you configured in Google Cloud Console
- What Ark sends in the OAuth request
- The actual callback URL Google redirects to

---
## 10. Common Troubleshooting
| Problem                      | Cause                                                       | Fix                                                        |
| ---------------------------- | ----------------------------------------------------------- | ---------------------------------------------------------- |
| 400 redirect_uri_mismatch    | Redirect URI not in credential config                       | Add exact URI to Web application OAuth client              |
| Token Exchange Failed        | Wrong application type or missing secret                    | Use Web application type; add client_secret if required    |
| ERR_INVALID_HTTP_RESPONSE    | HTTP/HTTPS mismatch in redirect                             | Ensure both server and OAuth redirect use HTTPS            |
| Missing email claim          | User hasn't made email public / scope missing               | Ensure `email` scope and maybe `profile`                   |
| 401 after callback           | ID token validation failure (clock skew or wrong client_id) | Sync system clock; verify client_id                        |
| Stale state                  | Pending auth purged or reused                               | Retry fresh login; avoid multi-start                       |
| No session cookie            | Domain/port mismatch or blocked cookies                     | Use consistent origin; disable third-party cookie blocking |
| Client authentication failed | Missing client_secret for Web app                           | Add client_secret to config for Web application type       |

---
## 11. Production Tips
- Enforce HTTPS and set `cookie_secure: true`.
- Consider rotating client secrets through environment variables if used.
- Limit scopes to least privilege (avoid broad Google API scopes unless required).
- Monitor logs for JWKS refresh; if blocked, allow outbound to `accounts.google.com`.

---
## 12. Multi-Provider Strategy
Ark currently selects one active provider via `auth.provider`. To switch between Microsoft and Google:
```yaml
auth:
  enabled: true
  provider: google   # or microsoft
  providers:
    - name: microsoft
      authority: "https://login.microsoftonline.com/<TENANT_ID>/v2.0"
      client_id: "<MS_CLIENT_ID>"
      client_secret: "<MS_CLIENT_SECRET>"  # Required for Web apps
      discovery: true
      scopes: "openid profile email"
    - name: google
      authority: "https://accounts.google.com"
      client_id: "<GOOGLE_CLIENT_ID>"
      client_secret: "<GOOGLE_CLIENT_SECRET>"  # Recommended for Web apps
      discovery: true
      scopes: "openid profile email"
```
Restart Ark after changing `auth.provider`.

---
## 13. Optional Enhancements
- Add audience override support if you start validating Access Tokens aimed at custom APIs.
- Implement token refresh (store refresh_token when offline_access used).
- Provide a UI selector to switch active provider (requires backend support to expose multiple providers concurrently).

---
## 14. Security Reminders
- Never commit client secrets to source control.
- Validate that the `iss` and `aud` claims match expectations; Ark already enforces issuer and audience (client_id) checks.
- Consider rate-limiting `/auth/login` to mitigate automated abuse.

---
## 15. Next Steps
- Set up Entra ID as an alternative (see `identity-entra-setup.md`).
- Add integration tests mocking JWKS for deterministic CI validation.

With this configuration in place, Google sign-in should function end-to-end.
