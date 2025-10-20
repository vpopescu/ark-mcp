use anyhow::{Context, Result};
use std::sync::Arc;
// Do not import StartupError here to avoid cross-crate path issues during
// mixed bin/lib compilation. Return an anyhow error with a clear marker
// string so the top-level binary can map it to a distinct exit code.
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_ENGINE;
use jsonwebtoken::{EncodingKey, Header};
use pem as pem_crate;
use sha2::{Digest, Sha256};
use simple_asn1::{ASN1Block, from_der};
use x509_parser::parse_x509_certificate;

#[derive(Clone)]
pub struct PemSigner {
    kid: String,
    encoding_key: EncodingKey,
    jwk: serde_json::Value,
}

pub trait Signer: Send + Sync {
    // TODO: implement this
    #[allow(dead_code)]
    fn kid(&self) -> &str;
    fn sign(&self, header: Header, claims: &serde_json::Value) -> Result<String>;
    fn jwks(&self) -> serde_json::Value;
}

impl PemSigner {
    pub fn from_pem(key_pem: &[u8], cert_pem: Option<&[u8]>) -> Result<Self> {
        // Try to parse private key as PKCS1 or PKCS8 using the rsa crate's
        // PEM parsers via the jsonwebtoken EncodingKey, but avoid depending
        // on the `rsa` crate types for Windows. We still use EncodingKey to
        // sign; extract the public modulus/exponent by parsing the PEM
        // ourselves using simple_asn1 where necessary.
        let encoding_key =
            EncodingKey::from_rsa_pem(key_pem).context("failed to create encoding key from pem")?;

        // Parse PEM to DER and extract modulus/exponent from private key
        let pem = pem_crate::parse(std::str::from_utf8(key_pem).context("key pem to str")?)
            .map_err(|e| anyhow::anyhow!("failed to parse key pem: {}", e))?;

        // Parse ASN.1 structure of private key (PKCS1 or PKCS8) to extract n,e
        let der = pem.contents;
        let asn1 = from_der(&der).context("failed to parse private key DER ASN.1")?;

        // Walk ASN.1 to locate integers for modulus and exponent
        fn extract_n_e(blocks: &[ASN1Block]) -> Option<(Vec<u8>, Vec<u8>)> {
            for b in blocks {
                if let ASN1Block::Sequence(_, inner) = b {
                    // PKCS1 private key: sequence of integers: 0, n, e, d, ...
                    if inner.len() >= 3
                        && let (ASN1Block::Integer(_, n), ASN1Block::Integer(_, e)) =
                            (&inner[1], &inner[2])
                    {
                        let n_bytes = n.to_signed_bytes_be();
                        let e_bytes = e.to_signed_bytes_be();
                        return Some((n_bytes, e_bytes));
                    }
                    // PKCS8 will embed algorithmIdentifier and privateKey as octet string
                    for elem in inner.iter() {
                        if let ASN1Block::OctetString(_, os) = elem
                            && let Ok(nested) = from_der(os)
                            && let Some((n, e)) = extract_n_e(&nested)
                        {
                            return Some((n, e));
                        }
                    }
                }
            }
            None
        }

        let (n_bytes, e_bytes) = extract_n_e(&asn1)
            .ok_or_else(|| anyhow::anyhow!("failed to extract n/e from private key ASN.1"))?;

        let n_b64 = BASE64_ENGINE.encode(&n_bytes);
        let e_b64 = BASE64_ENGINE.encode(&e_bytes);

        // Compute kid as SHA-256(n || e)
        let mut hasher = Sha256::new();
        hasher.update(&n_bytes);
        hasher.update(&e_bytes);
        let kid = hex::encode(hasher.finalize());

        let jwk = serde_json::json!({
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": kid,
            "n": n_b64,
            "e": e_b64,
        });

        // If a certificate was provided, parse it and ensure the public key
        // in the cert matches the private key we just parsed. This prevents
        // accidental mismatches (wrong file paths) and ensures the JWKS we
        // publish corresponds to the private key used for signing.
        if let Some(cert_bytes) = cert_pem {
            // parse PEM wrapper (may contain additional certs)
            let pem = pem_crate::parse(std::str::from_utf8(cert_bytes).context("cert pem to str")?)
                .map_err(|e| anyhow::anyhow!("failed to parse cert pem: {}", e))?;
            let der = pem.contents;

            let (_, cert) = parse_x509_certificate(&der)
                .map_err(|e| anyhow::anyhow!("failed to parse x509 certificate: {:?}", e))?;

            let subject_pki = &cert.tbs_certificate.subject_pki.subject_public_key.data;

            // Parse subject_pki DER and extract the RSA modulus/exponent
            let subject_pki_asn = from_der(subject_pki)
                .map_err(|_| anyhow::anyhow!("failed to parse subject_pki DER"))?;
            // subject_pki is the DER of RSAPublicKey: SEQUENCE { INTEGER n, INTEGER e }
            let mut pub_n = None;
            let mut pub_e = None;
            if subject_pki_asn.len() == 1
                && let ASN1Block::Sequence(_, seq) = &subject_pki_asn[0]
                && seq.len() >= 2
                && let (ASN1Block::Integer(_, n), ASN1Block::Integer(_, e)) = (&seq[0], &seq[1])
            {
                pub_n = Some(n.to_signed_bytes_be());
                pub_e = Some(e.to_signed_bytes_be());
            }

            let pub_n = pub_n.ok_or_else(|| {
                anyhow::anyhow!("failed to extract public modulus from cert subject_pki")
            })?;
            let pub_e = pub_e.ok_or_else(|| {
                anyhow::anyhow!("failed to extract public exponent from cert subject_pki")
            })?;

            // Compare modulus/exponent
            if pub_n != n_bytes || pub_e != e_bytes {
                return Err(anyhow::anyhow!(
                    "KeyCertMismatch: Certificate public key does not match provided private key"
                ));
            }
        }

        Ok(PemSigner {
            kid,
            encoding_key,
            jwk,
        })
    }
}

impl Signer for PemSigner {
    fn kid(&self) -> &str {
        &self.kid
    }

    fn sign(&self, mut header: Header, claims: &serde_json::Value) -> Result<String> {
        header.kid = Some(self.kid.clone());
        let token = jsonwebtoken::encode(&header, claims, &self.encoding_key)
            .context("failed to encode jwt")?;
        Ok(token)
    }

    fn jwks(&self) -> serde_json::Value {
        serde_json::json!({ "keys": [self.jwk.clone()] })
    }
}

pub type DynSigner = Arc<dyn Signer>;

pub fn load_pem_signer_from_paths(key_path: &str, cert_path: Option<&str>) -> Result<DynSigner> {
    let key = std::fs::read(key_path).context("read private key")?;
    let cert = match cert_path {
        Some(p) => Some(std::fs::read(p).context("read cert")?),
        None => None,
    };
    let cert_ref = cert.as_deref();
    let signer = PemSigner::from_pem(&key, cert_ref).context("create pem signer")?;
    Ok(Arc::new(signer))
}
