use crate::types::TraceInfo;
use std::time::Instant;

pub async fn measure_latency(socks_port: u16) -> Result<u64, String> {
    let proxy_url = format!("socks5h://127.0.0.1:{socks_port}");
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).map_err(|e| e.to_string())?)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let start = Instant::now();
    let resp = client
        .get("http://www.gstatic.com/generate_204")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status().is_success() || resp.status().as_u16() == 204 {
        Ok(start.elapsed().as_millis() as u64)
    } else {
        Err(format!("Unexpected probe status: {}", resp.status()))
    }
}

pub async fn fetch_trace(socks_port: u16) -> Result<TraceInfo, String> {
    let proxy_url = format!("socks5h://127.0.0.1:{socks_port}");
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).map_err(|e| e.to_string())?)
        .timeout(std::time::Duration::from_secs(7))
        .build()
        .map_err(|e| e.to_string())?;

    let text = client
        .get("https://www.cloudflare.com/cdn-cgi/trace")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let mut ip = String::from("Unknown");
    let mut loc = String::from("Unknown");
    let mut colo = String::from("Unknown");
    let mut warp = String::from("off");

    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "ip" => ip = v.trim().to_string(),
                "loc" => loc = v.trim().to_string(),
                "colo" => colo = v.trim().to_string(),
                "warp" => warp = v.trim().to_string(),
                _ => {}
            }
        }
    }

    Ok(TraceInfo { ip, loc, colo, warp })
}
