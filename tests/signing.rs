use jsonwebtoken::jwk::JwkSet;

// Use a small, static RSA private key PEM for tests to avoid depending on the `rsa` crate.
// This key is only for unit tests and not used in production.
const TEST_RSA_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----\nMIIEogIBAAKCAQEA2cXCBpYcHvqxgXvGSO7mfaJ9eo6YkkZ+4NJ9Jvm34AGiSvkC\nXR7Jyilmu3OuUw+cTrUamkczN/PKgYAX2smv/MiSBlPDZ/wZEjgWGu5bAnX4CXjG\nf247SZC1jgifhCOSCvF70fe0UkdPM8rhq1U8Os6XFh3jKsxc7mJlD7Cpn9Hj9RCK\nSMqIIPKSeqb5K+YBdSe/CRl0Yrrvnr7CLzNX9WM3Sy+New5YrW4C4Vxgwe4R5jIZ\n1BrdHGkMxO3LK+/7dOzmBe7orAchOic0EcsnYI3Dgsu90UaZM4YOipD94/7fLA/d\ncHpF6ClvPGqn33sxw21/Qr34BXmDsZSv+3YUXQIDAQABAoIBAAj6D2jxGCCoyddi\nEEbiXirwn0aFiUGCWWmQE6ufIJbBynxXrmLDSaMlOMBiYN24p4NRENMqOzDDwmW7\nL4CLzs7XP7m2CziGmkv3duXOTH8Z+Mr/KQOIujXqmqlLrrOmObdsw+NgWBUKLnge\nlVXYMh7kdDLrsXkKqowDD1JpwFw/mbNccMF390slr5nopxFAqREAWlylHKZQlGE5\nBA2959IBdterY8OmowtBnMhevgyfPxqMoJ1mdDFZgE1lbiAC9plNR6S9uOSz6Zou\nGT6o7zktY5of/SesUFe2IOazUrDxhDv85mQNBWcmt7aB+JzsPpnsw8+hcfdeMACi\nVxWoqYECgYEA8G3fvwd0a8BaXKpig51MDnlrdSoTrYiCIB4NpakTeGdli+z5LX5v\nWXVmGtRQLGoN7bthlgLIxA63NCA6dTex/3UoiXnxXWuNwu2BWc4mu4ta/UmxQSid\nR4JDpn5BM+bRQqQV4TCa0YqbHSUhVn6xH86AcVI29tqvvsqLQF/zD50CgYEA5+BA\n8JmiOjd4QHcemp43LT8qmsxbXdDFd3CjuUiUkSW5vUPw98q8+IA5juGk69zw5I3F\njeg/IvmKFEIsWFQiEQLbub5SZAcE7JN2qwxU4SNFjuFwmWdfi5b3M7VMe4xL90P4\nz/Pt6QBYjlaeEjqpO5aynHjFZXF3Vp3A09GM28ECgYB6POKNFRUz01Ad3OLJV6fb\nlA/2ObZXfBfsjFsT5qpnhOo0Af+OCcJDEVUgPuGkMydxvtsWkcPRKkoqzlfqUK7G\n2qIJg14byRsCCA7DwfQfVfKk5FqibivIt4n9lCNCaA/sedBF9ZhBAN9sKfyRJUiY\nizzyYIJhbz37Gq9Bw4aoYQKBgFKZrkiHUiUO8YV1aa+GwP0bTWALgFixMEbWF1y/\noDz8hWgIteRvklWrx9VASHHFKQMiBcgBfcxFvIxu7kEg52nL7N4EEHGVlol4FoPk\nRrBU0kiNwoDDNGQTiUggQ3iXh9AzpITfzlZ8Sw+Zh4HS58pUapgW5aq3et2eILzU\nHyuBAoGALmm40jwKQxWQp7xWgnn9OqXMLdrhTRBL62PIG9Q7Hjul9hdGj0JYeOeU\nR+WCYyYSZt5+1PUGPYsMDWnbCMeYYt2ky6354/8zFW2T8j8UU+fMpnKCcDxkwUt6\n1jCPSYoanj0w9Cyr0iSQaXnAakyhVwv5auk/t6lZXCozkDgfdTo=\n-----END RSA PRIVATE KEY-----\n";

#[test]
fn pem_sign_and_verify() {
    // Build signer from static PEM bytes
    let signer = ark::server::signing::PemSigner::from_pem(TEST_RSA_PEM.as_bytes(), None)
        .expect("create signer");
    let dyn_signer: ark::server::signing::DynSigner = std::sync::Arc::new(signer);

    let claims = serde_json::json!({"sub":"user1","aud":"client","exp":9999999999u64,"iat":1u64});
    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    let token = dyn_signer.sign(header, &claims).expect("sign");

    // Build JwkSet and decode using jsonwebtoken
    let jwks = dyn_signer.jwks();
    let jwk_set: JwkSet = serde_json::from_value(jwks).expect("jwk_set");
    let decoding = jsonwebtoken::DecodingKey::from_jwk(&jwk_set.keys[0]).expect("decoding key");
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_audience(&["client"]);
    let data =
        jsonwebtoken::decode::<serde_json::Value>(&token, &decoding, &validation).expect("decode");
    assert_eq!(data.claims.get("sub").unwrap(), "user1");
}
