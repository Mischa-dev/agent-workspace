use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand_core::OsRng;
use zeroize::Zeroize;

use crate::error::AppError;

const MEMORY_KIB: u32 = 65_536;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;
const OUTPUT_BYTES: usize = 32;

fn argon2() -> Result<Argon2<'static>, AppError> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, Some(OUTPUT_BYTES))
        .map_err(|_| AppError::Internal("construct Argon2 parameters"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

pub async fn hash_password(mut password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            let salt = SaltString::generate(&mut OsRng);
            argon2()?
                .hash_password(password.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|_| AppError::Internal("hash administrator password"))
        })();
        password.zeroize();
        result
    })
    .await
    .map_err(|_| AppError::Internal("join password hashing worker"))?
}

pub async fn verify_password(mut password: String, password_phc: String) -> Result<bool, AppError> {
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            let parsed = PasswordHash::new(&password_phc)
                .map_err(|_| AppError::Internal("parse stored password hash"))?;
            Ok(argon2()?
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        })();
        password.zeroize();
        result
    })
    .await
    .map_err(|_| AppError::Internal("join password verification worker"))?
}

pub async fn verify_unknown_login(mut password: String) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            let salt = SaltString::encode_b64(b"agent-workspace-login-dummy")
                .map_err(|_| AppError::Internal("construct dummy password salt"))?;
            // A known-account failure performs one Argon2id KDF through password
            // verification. Unknown accounts perform exactly one equivalent-cost
            // KDF here so the dummy path does not double its time or memory cost.
            argon2()?
                .hash_password(password.as_bytes(), &salt)
                .map(|_| ())
                .map_err(|_| AppError::Internal("run dummy login password KDF"))
        })();
        password.zeroize();
        result
    })
    .await
    .map_err(|_| AppError::Internal("join dummy password worker"))?
}

#[cfg(test)]
mod tests {
    use super::{hash_password, verify_password, verify_unknown_login};

    #[tokio::test]
    async fn stores_argon2id_phc_and_verifies_without_plaintext() {
        let password = "correct horse battery staple".to_owned();
        let phc = hash_password(password.clone())
            .await
            .expect("hash password");
        assert!(phc.starts_with("$argon2id$v=19$m=65536,t=3,p=1$"));
        assert!(!phc.contains(&password));
        assert!(
            verify_password(password, phc.clone())
                .await
                .expect("verify password")
        );
        assert!(
            !verify_password("wrong password".to_owned(), phc)
                .await
                .expect("reject password")
        );
    }

    #[tokio::test]
    async fn unknown_login_runs_the_bounded_dummy_kdf() {
        verify_unknown_login("correct horse battery staple".to_owned())
            .await
            .expect("dummy verification path");
    }
}
