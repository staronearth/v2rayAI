use aes::Aes256;
use base64::Engine;
use cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

type HmacSha256 = Hmac<Sha256>;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

const SERVICE: &str = "v2rayAI";
const ACCOUNT: &str = "ai-api-key-master";

#[derive(Debug, Serialize, Deserialize)]
struct SecretFile {
    version: u8,
    nonce: String,
    ciphertext: String,
    tag: String,
}

pub async fn save_ai_api_key(api_key: String) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return clear_ai_api_key().await;
    }

    let master = get_or_create_master_key().await?;
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);

    let ciphertext = aes_ctr_crypt(&derive_key(&master, b"enc"), &nonce, api_key.as_bytes());
    let tag = sign(&derive_key(&master, b"mac"), &nonce, &ciphertext)?;
    let file = SecretFile {
        version: 1,
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
        tag: B64.encode(tag),
    };

    let path = secret_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建密钥目录失败：{}", e))?;
    }
    let json = serde_json::to_string_pretty(&file).map_err(|e| format!("序列化密钥失败：{}", e))?;
    tokio::fs::write(path, json)
        .await
        .map_err(|e| format!("保存加密密钥失败：{}", e))
}

pub async fn load_ai_api_key() -> Result<Option<String>, String> {
    let path = secret_path();
    if !path.exists() {
        return Ok(None);
    }

    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("读取加密密钥失败：{}", e))?;
    let file: SecretFile =
        serde_json::from_str(&text).map_err(|e| format!("解析加密密钥失败：{}", e))?;
    if file.version != 1 {
        return Err("不支持的密钥文件版本".to_string());
    }

    let master = get_or_create_master_key().await?;
    let nonce = B64
        .decode(file.nonce)
        .map_err(|e| format!("nonce 解码失败：{}", e))?;
    let ciphertext = B64
        .decode(file.ciphertext)
        .map_err(|e| format!("密文解码失败：{}", e))?;
    let tag = B64
        .decode(file.tag)
        .map_err(|e| format!("签名解码失败：{}", e))?;
    let expected = sign(&derive_key(&master, b"mac"), &nonce, &ciphertext)?;
    if !constant_time_eq(&tag, &expected) {
        return Err("AI Key 密文校验失败".to_string());
    }

    let plaintext = aes_ctr_crypt(&derive_key(&master, b"enc"), &nonce, &ciphertext);
    String::from_utf8(plaintext)
        .map(Some)
        .map_err(|e| format!("AI Key 解密结果不是 UTF-8：{}", e))
}

pub async fn clear_ai_api_key() -> Result<(), String> {
    match tokio::fs::remove_file(secret_path()).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除加密密钥失败：{}", e)),
    }
}

fn secret_path() -> PathBuf {
    crate::config_manager::dirs_for_app()
        .join("secure")
        .join("ai-api-key.enc.json")
}

async fn get_or_create_master_key() -> Result<Vec<u8>, String> {
    if let Some(key) = read_master_key().await? {
        return Ok(key);
    }

    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    write_master_key(&key).await?;
    Ok(key.to_vec())
}

#[cfg(target_os = "macos")]
async fn read_master_key() -> Result<Option<Vec<u8>>, String> {
    let output = tokio::process::Command::new("security")
        .args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"])
        .output()
        .await
        .map_err(|e| format!("读取 Keychain 失败：{}", e))?;

    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    B64.decode(text)
        .map(Some)
        .map_err(|e| format!("Keychain 主密钥解码失败：{}", e))
}

#[cfg(target_os = "macos")]
async fn write_master_key(key: &[u8]) -> Result<(), String> {
    let secret = B64.encode(key);
    let output = tokio::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            SERVICE,
            "-a",
            ACCOUNT,
            "-w",
            &secret,
        ])
        .output()
        .await
        .map_err(|e| format!("写入 Keychain 失败：{}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "写入 Keychain 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(not(target_os = "macos"))]
async fn read_master_key() -> Result<Option<Vec<u8>>, String> {
    let path = crate::config_manager::dirs_for_app()
        .join("secure")
        .join("master.key");
    if !path.exists() {
        return Ok(None);
    }
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("读取本地主密钥失败：{}", e))?;
    B64.decode(text.trim())
        .map(Some)
        .map_err(|e| format!("本地主密钥解码失败：{}", e))
}

#[cfg(not(target_os = "macos"))]
async fn write_master_key(key: &[u8]) -> Result<(), String> {
    let path = crate::config_manager::dirs_for_app()
        .join("secure")
        .join("master.key");
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建主密钥目录失败：{}", e))?;
    }
    tokio::fs::write(&path, B64.encode(key))
        .await
        .map_err(|e| format!("写入本地主密钥失败：{}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn derive_key(master: &[u8], label: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(master);
    hasher.update(label);
    hasher.finalize().into()
}

fn aes_ctr_crypt(key: &[u8; 32], nonce: &[u8], input: &[u8]) -> Vec<u8> {
    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut output = Vec::with_capacity(input.len());
    let mut counter: u64 = 0;

    for chunk in input.chunks(16) {
        let mut block = [0u8; 16];
        block[..8].copy_from_slice(&nonce[..8]);
        block[8..].copy_from_slice(&counter.to_be_bytes());
        let mut block = GenericArray::clone_from_slice(&block);
        cipher.encrypt_block(&mut block);
        output.extend(chunk.iter().zip(block.iter()).map(|(a, b)| a ^ b));
        counter = counter.wrapping_add(1);
    }
    output
}

fn sign(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(key).map_err(|e| format!("创建 HMAC 失败：{}", e))?;
    mac.update(nonce);
    mac.update(ciphertext);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
