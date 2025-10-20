# Sample environment variables for Google OAuth authentication
# Copy this file and update the values with your actual Google OAuth configuration

# Enable authentication
$env:ARK_AUTH_ENABLED = "true"

# Set the provider to Google
$env:ARK_AUTH_PROVIDER = "google"

# Your Google OAuth client ID
$env:ARK_AUTH_CLIENT_ID = "your-google-client-id.apps.googleusercontent.com"

# Your Google OAuth client secret (keep this secure!)
$env:ARK_AUTH_CLIENT_SECRET = "your-google-client-secret"

# Google OAuth authority URL
# Standard Google OAuth endpoint (can be customized for different environments)
$env:ARK_AUTH_AUTHORITY = "https://accounts.google.com"

# OAuth scopes to request (optional, defaults to "openid profile email")
# $env:ARK_AUTH_SCOPES = "openid profile email"

# TLS configuration (optional)
# $env:ARK_TLS_CERT = "C:\ProgramData\ark\tls\cert.pem"
# $env:ARK_TLS_KEY = "C:\ProgramData\ark\tls\key.pem"
# $env:ARK_TLS_SILENT_INSECURE = "false"

# Group emails for role mapping (optional)
$env:ARK_AUTH_ADMIN_GROUP = "ark-administrators@yourdomain.example"
$env:ARK_AUTH_USER_GROUP = "ark-users@yourdomain.example"
