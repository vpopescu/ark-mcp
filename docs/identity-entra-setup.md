# Entra ID (Azure AD) Integration Guide — 2025 Portal UI

This guide walks you through creating an Entra ID application, configuring redirect URIs, collecting required identifiers, and wiring them into the Ark server `auth` configuration and frontend (Node/Vite) app.

---

## 1. Terminology

- **Tenant ID**: Found under **Microsoft Entra admin center → Identity → Overview**
- **Application (Client) ID**: Found in **App registrations → Overview**
- **Redirect URI**: Configured under **Authentication → Platform configurations**
- **Public vs Confidential App**: SPA = public (no secret), Web = confidential (requires secret)
- **PKCE**: Supported natively for SPA apps; no extra config needed

---

## 2. Create an Application Registration

1. Go to [https://entra.microsoft.com](https://entra.microsoft.com)
2. Navigate: **Applications → App registrations → + New registration**
3. Name: `Ark MCP Admin Console` or your preferred name
4. Supported account types: Choose **Single tenant** for dev
5. Redirect URI: Skip or add later under **Platform configurations**
6. Click **Register**

After registration, copy:
- **Directory (tenant) ID**
- **Application (client) ID**

These map to:
- `tenant_id` → used in authority URL
- `client_id` → used in config


> Note: this **creates a registration for the console app**. You may want to 
create an additional registration for any other app you may want accessing the APIs. Depending on your application type (SPA vs other types), it may need to be configured slightly different below.

---

## 3. Configure Authentication (Redirect URIs)

1. Go to **Authentication** tab in your app registration
2. Under **Platform configurations**, click **+ Add a platform**
3. **IMPORTANT**: Choose **Web** (not Single-page application)
   - Web applications support server-side token exchange
   - SPA applications only support client-side flows and will cause token exchange errors
4. Add redirect URIs under the **Web** platform:
   - `http://localhost:8000/auth/callback`  (for local development)
   - `https://localhost:8000/auth/callback` (for local HTTPS development)
   - `https://127.0.0.1:8000/auth/callback` (alternative for local HTTPS)
   - (Optional) `https://your-domain.example.com/auth/callback` (for production)
5. Add **Front-channel logout URL**:
   - `https://localhost:8000/auth/logout` (note: HTTP is not supported here)
6. Under **Implicit grant and hybrid flows**:
   - ✅ **UNCHECK "Access tokens (used for implicit flows)"**
   - ✅ **UNCHECK "ID tokens (used for implicit and hybrid flows)"**
7. Under **Advanced settings**:
   - ✅ **Set "Allow public client flows" to "No"**
8. Click **Save**

**For production deployments:**
- Go to **Certificates & secrets → + New client secret**
- Save the **Value** securely (you'll need this for the `client_secret` config)
- Client secrets provide additional security for server-side applications

---

## 4. API Permissions in config file

Default OpenID scopes are sufficient:
- `openid`
- `profile`
- `email`

You mway want to add `offline-access` if your application needs it

---

## 5. Authority URL 

Use:
```
https://login.microsoftonline.com/<TENANT_ID>/v2.0
```

Ark uses this for discovery and token exchange.

---

## 6. Ark Configuration Snippet

```yaml
auth:
  enabled: true
  provider: microsoft
  session:
    timeout_seconds: 3600
    cookie_name: ark_session
    cookie_secure: true      # Set to true for HTTPS (recommended)
    cookie_http_only: true
    same_site: Lax
  providers:
    - name: microsoft
      authority: "https://login.microsoftonline.com/<TENANT_ID>/v2.0"
      client_id: "<APPLICATION_CLIENT_ID>"
      discovery: true
      scopes: "openid profile email"
      # client_secret: "<CLIENT_SECRET>"  # Required for production Web apps
```

**Notes:**
- For local development with HTTPS, set `cookie_secure: true`
- For local development with HTTP only, set `cookie_secure: false`
- The `client_secret` is optional for development but recommended for production

---

## 7. Frontend Flow

- `GET /auth/status` → check login
- `GET /auth/login` → returns redirect URL
- Browser → Entra login → `/auth/callback`
- Ark exchanges code, validates token, sets cookie

---

## 8. Local Testing

1. Start Ark on `https://localhost:8000` (with TLS enabled)
2. Access the login at `https://127.0.0.1:8000/auth/login` or `https://localhost:8000/auth/login`
3. Trigger login → check redirect includes:
   - `client_id`
   - `redirect_uri` (should use HTTPS)
   - `code_challenge` + `S256`
4. Sign in → verify `ark_session` cookie is set
5. Check that callback URL uses HTTPS (not HTTP)

**Common Issues:**
- If you see `ERR_INVALID_HTTP_RESPONSE`, the callback is using HTTP instead of HTTPS
- If you see `AADSTS9002327`, your app is configured as SPA instead of Web

---

## 9. Troubleshooting

| Problem                          | Cause                    | Fix                                               |
| -------------------------------- | ------------------------ | ------------------------------------------------- |
| AADSTS500113                     | Redirect URI mismatch    | Add exact URI under Web platform configurations   |
| AADSTS9002327                    | App configured as SPA    | Change platform from SPA to Web in Authentication |
| ERR_INVALID_HTTP_RESPONSE        | HTTP/HTTPS mismatch      | Ensure both server and redirect URI use HTTPS     |
| Missing `authorization_endpoint` | Discovery race           | Restart Ark; retry login                          |
| Token Exchange Failed            | Wrong app type           | Verify app is configured as Web, not SPA          |
| Implicit grant warning           | Legacy flows enabled     | Uncheck both implicit grant checkboxes            |
| Public client flows enabled      | Wrong flow type          | Set "Allow public client flows" to "No"           |
| 401 after callback               | Token validation failure | Check tenant/client IDs and time sync             |
| Cookie not present               | HTTPS or SameSite issues | Use `cookie_secure: true`; verify domain          |
| Infinite redirect loop           | Session lost or blocked  | Check cookies and callback logs                   |

---

## 10. Production Tips

- Use HTTPS and `cookie_secure: true`
- Consider `SameSite: Strict`
- Rotate secrets via environment variables
- Monitor JWKS refresh logs

---

## 11. Production Web App Example

```yaml
auth:
  enabled: true
  provider: microsoft
  session:
    timeout_seconds: 3600
    cookie_name: ark_session
    cookie_secure: true      # Required for HTTPS
    cookie_http_only: true
    same_site: Strict        # More secure for production
  providers:
    - name: microsoft
      authority: "https://login.microsoftonline.com/<TENANT_ID>/v2.0"
      client_id: "<CLIENT_ID>"
      client_secret: "<CLIENT_SECRET>"  # Required for Web apps in production
      discovery: true
      scopes: "openid profile email"
```

---

## 12. Next Steps

- Add Google Identity Platform (`identity-google-setup.md`)
- Implement logout propagation if needed

Adapt scopes and cookie policies to your security model.