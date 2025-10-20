# Sample environment variables for Microsoft Entra ID authentication
# Copy this file and update the values with your actual Entra ID configuration

# Enable authentication
$env:ARK_AUTH_ENABLED = "true"

# Set the provider to Microsoft Entra ID
$env:ARK_AUTH_PROVIDER = "microsoft"

# Your Entra ID application client ID
$env:ARK_AUTH_CLIENT_ID = "fd9bf055-5f72-49af-9bd7-c40905586de9"

# Your Entra ID client secret (keep this secure!)
$env:ARK_AUTH_CLIENT_SECRET = "your-client-secret-here"

# Your Entra ID authority URL (full OIDC endpoint)
# For commercial Microsoft: https://login.microsoftonline.com/{tenant-id}/v2.0
# For Azure Government: https://login.microsoftonline.us/{tenant-id}/v2.0
# For Azure China: https://login.chinacloudapi.cn/{tenant-id}/v2.0
$env:ARK_AUTH_AUTHORITY = "https://login.microsoftonline.com/24e533d9-0000-0000-0000-f76ece3ee6e2/v2.0"

# OAuth scopes to request (optional, defaults to "openid profile email")
# $env:ARK_AUTH_SCOPES = "openid profile email"

# TLS configuration (optional)
# $env:ARK_TLS_CERT = "C:\ProgramData\ark\tls\cert.pem"
# $env:ARK_TLS_KEY = "C:\ProgramData\ark\tls\key.pem"
# $env:ARK_TLS_SILENT_INSECURE = "false"

# Group IDs for role mapping (optional)
$env:ARK_AUTH_ADMIN_GROUP = "e7f3a8a8-8b3a-4f88-9c3a-0123456789ab"
$env:ARK_AUTH_USER_GROUP = "b12c3456-789d-4e2f-a123-456789abcdef"
