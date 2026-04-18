use dotenv::dotenv;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::{env, fs::OpenOptions, io::Write, time::Duration};
use tokio::time::sleep;
use uuid::Uuid;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use chrono::Local;
use sha2::{Sha512, Digest};

/* 변수 설정 메인에 넣음
let query = "market=KRW-BTC&side=bid&price=10000&ord_type=price";
let hash = Sha512::digest(query.as_bytes());
let query_hash = format!("{:x}", hash);
*/
// --- 업비트 API 응답 구조체 ---
#[derive(Debug, Serialize)]
struct Claims {
    access_key: String,
    nonce: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    query_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query_hash_alg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Balance {
    currency: String,
    balance: String,
    avg_buy_price: String,
}

#[derive(Debug, Deserialize, Clone)]
struct UpbitCandle {
    trade_price: f64,
    opening_price: f64,
    candle_acc_trade_volume: f64,
}

// --- 유틸리티: 인증 토큰 생성 ---
fn create_token_with_query(
    access_key: &str,
    secret_key: &str,
    query: &str,
) -> String {
    // SHA512 해시 생성
    let hash = Sha512::digest(query.as_bytes());
    let query_hash = hex::encode(hash);

    let claims = Claims {
        access_key: access_key.to_string(),
        nonce: Uuid::new_v4().to_string(),
        query_hash: Some(query_hash),
        query_hash_alg: Some("SHA512".to_string()),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret_key.as_bytes()),
    )
    .unwrap()
}
// --- 유틸리티: 인증 토큰 생성 (쿼리 없는 경우) ---
fn create_token(access_key: &str, secret_key: &str) -> String {
    let claims = Claims {
        access_key: access_key.to_string(),
        nonce: Uuid::new_v4().to_string(),
        query_hash: None,
        query_hash_alg: None,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret_key.as_bytes()),
    )
    .unwrap()
}

// --- 유틸리티: 로그 기록 및 디스코드 알림 통합 ---
async fn alert(message: &str) {
    let datetime = Local::now().format("%Y-%m-%d %H:%M:%S");
    let formatted_msg = format!("[{}] {}", datetime, message);
    
    // 1. 콘솔 출력
    println!("{}", formatted_msg);

    // 2. 파일 로그 기록 (trading.log)
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open("trading.log") {
        let _ = writeln!(file, "{}", formatted_msg);
    }

    // 3. 디스코드 전송
    let webhook_url = env::var("DISCORD_WEBHOOK_URL").unwrap_or_default();
    if !webhook_url.is_empty() {
        let client = reqwest::Client::new();
        let _ = client.post(webhook_url).json(&serde_json::json!({ "content": message })).send().await;
    }
}

// --- 기술 지표 계산 로직 ---
fn calculate_ema(prices: &[f64], period: usize) -> f64 {
    if prices.len() < period { return *prices.last().unwrap_or(&0.0); }
    let multiplier = 2.0 / (period as f64 + 1.0);
    let mut ema = prices[0..period].iter().sum::<f64>() / period as f64;
    for price in prices.iter().skip(period) {
        ema = (price - ema) * multiplier + ema;
    }
    ema
}

fn calculate_rsi(prices: &[f64], period: usize) -> f64 {
    if prices.len() <= period { return 50.0; }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for i in (prices.len() - period + 1)..prices.len() {
        let change = prices[i] - prices[i - 1];
        if change > 0.0 { gains += change; } else { losses -= change; }
    }
    let rs = (gains / period as f64) / (losses / period as f64).max(0.00001);
    100.0 - (100.0 / (1.0 + rs))
}
//gpt사고팔기 로직 
async fn buy_order(
    client: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    amount: f64,
) {
    let query = format!(
        "market=KRW-BTC&side=bid&price={}&ord_type=price",
        amount
    );

    let token = create_token_with_query(access_key, secret_key, &query);

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
    );

    let params = [
        ("market", "KRW-BTC"),
        ("side", "bid"),
        ("price", &amount.to_string()),
        ("ord_type", "price"),
    ];

    match client
        .post("https://api.upbit.com/v1/orders")
        .headers(headers)
        .form(&params)
        .send()
        .await
    {
        Ok(res) => {
            let text: String = res.text().await.unwrap_or_default();

            alert(&format!(
                "🟢 **매수 체결 시도**\n- 금액: {:.0}원\n- 응답: {}",
                amount, text
            )).await;
        }
        Err(e) => {
            alert(&format!("❌ 매수 실패: {}", e)).await;
        }
    }
}

async fn sell_order(
    client: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    volume: f64,
) {
    let query = format!(
        "market=KRW-BTC&side=ask&volume={}&ord_type=market",
        volume
    );

    let token = create_token_with_query(access_key, secret_key, &query);

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
    );

    let params = [
        ("market", "KRW-BTC"),
        ("side", "ask"),
        ("volume", &volume.to_string()),
        ("ord_type", "market"),
    ];

    match client
        .post("https://api.upbit.com/v1/orders")
        .headers(headers)
        .form(&params)
        .send()
        .await
    {
        Ok(res) => {
            let text: String = res.text().await.unwrap_or_default();

            alert(&format!(
                "🔴 **매도 체결 시도**\n- 수량: {:.6}\n- 응답: {}",
                volume, text
            )).await;
        }
        Err(e) => {
            alert(&format!("❌ 매도 실패: {}", e)).await;
        }
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    let access_key = env::var("UPBIT_ACCESS_KEY").expect("ACCESS_KEY missing");
    let secret_key = env::var("UPBIT_SECRET_KEY").expect("SECRET_KEY missing");
    let client = reqwest::Client::new();
    let mut log_counter = 0; //콘솔 출력 카운터
    let mut highest_held_price: f64 = 0.0;//매수이후 최고점 저장 변수

    alert("🚀 **자동 매매 시스템이 가동되었습니다!** (전략: EMA+RSI 하이브리드)").await;

    loop {
        
        // --- 1. 시세 조회 (15분봉 100개) ---
        let candle_url = "https://api.upbit.com/v1/candles/minutes/15?market=KRW-BTC&count=100";
        let res = match client.get(candle_url).send().await {
            Ok(r) => r,
            Err(e) => {
                println!("⚠️ 시세 조회 실패: {}. 30초 후 재시도.", e);
                sleep(Duration::from_secs(30)).await;
                continue;
            }
        };

        if let Ok(mut candles) = res.json::<Vec<UpbitCandle>>().await {
            candles.reverse();
            let prices: Vec<f64> = candles.iter().map(|c| c.trade_price).collect();
            let current_candle = candles.last().unwrap();
            let current_price = current_candle.trade_price;
            let current_open = current_candle.opening_price;
            let current_volume = current_candle.candle_acc_trade_volume;
            let ema20 = calculate_ema(&prices, 20);
            let ema50 = calculate_ema(&prices, 50);
            //최근20개 캔들의 평균 계산량 계산
            let vol_sma20: f64 = candles.iter().rev().skip(1).take(20).map(|c| c.candle_acc_trade_volume).sum::<f64>() / 20.0;
            // 현재 RSI와 직전 캔들의 RSI를 비교하기 위해 두 개를 계산
            let rsi_current = calculate_rsi(&prices, 14);
            let rsi_prev = if prices.len() > 1 {
                calculate_rsi(&prices[..prices.len()-1], 14)
            } else {
                50.0
            };  
            
            //300초당 1회 콘솔 출력 (너무 자주 출력하면 로그가 지저분해질 수 있어서) + 시간출력
            log_counter += 1;
            if log_counter >= 10 {
                let now = Local::now().format("%H:%M");

                println!(
                    "[{}] 📊 price: {:.0} | EMA20: {:.2} | EMA50: {:.2} | RSI14: {:.2}",
                    now, current_price, ema20, ema50, rsi_current
                );

                log_counter = 0;
            }

            // --- 2. 잔고 조회 ---
            let token = create_token(&access_key, &secret_key);
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

            let Ok(bal_res) = client.get("https://api.upbit.com/v1/accounts").headers(headers).send().await else { continue; };
            let balances = match bal_res.json::<Vec<Balance>>().await {
                Ok(b) => b,
                Err(_) => continue,
            };

            let krw_val: f64 = balances.iter().find(|b| b.currency == "KRW").map(|b| b.balance.parse().unwrap_or(0.0)).unwrap_or(0.0);
            let btc_amount: f64 = balances.iter().find(|b| b.currency == "BTC").map(|b| b.balance.parse().unwrap_or(0.0)).unwrap_or(0.0);
            let btc_avg_price: f64 = balances.iter().find(|b| b.currency == "BTC").map(|b| b.avg_buy_price.parse().unwrap_or(0.0)).unwrap_or(0.0);

            let btc_eval_val = btc_amount * current_price;
            let total_assets = krw_val + btc_eval_val;
            let current_profit_pct = if btc_avg_price > 0.0 { (current_price - btc_avg_price) / btc_avg_price * 100.0 } else { 0.0 };

            // --- 💡 트레일링 스탑 상태 업데이트 ---
            if btc_amount > 0.0001 {
                if current_price > highest_held_price {
                    highest_held_price = current_price; // 코인을 들고 있는 동안 최고점 갱신
                }
            } else {
                highest_held_price = 0.0; // 코인이 없으면 초기화
            }

            // --- 3. 매수/매도 로직 판단 (가상 매매 모드) ---
            // A. 매수 로직 (NEW 로직)
            let is_uptrend = ema20 > ema50;
            let is_green_candle = current_price > current_open; // 양봉 확인
            let is_rsi_hook = rsi_prev < 40.0 && rsi_current > rsi_prev; // RSI가 40 미만에서 위로 꺾임
            let is_vol_spike = current_volume > (vol_sma20 * 1.5); // 평균 거래량 대비 1.5배 터짐

            if is_uptrend && is_rsi_hook && is_green_candle && is_vol_spike && btc_eval_val < (total_assets * 0.8) {

                let rsi_weight = 1.0 + (40.0 - rsi_current.max(20.0)) / 20.0;

                let buy_amount = (10000.0 * rsi_weight).min(krw_val * 0.9);

                if buy_amount >= 5000.0 {
                    // 대신 디스코드 알림만 전송합니다.
                        alert(&format!(
                        "🧪 **[매수 신호 - 바닥 확인!]**\n- 💰 예상 금액: {:.0}원\n- 📊 RSI 훅: {:.2} ➡️ {:.2}\n- 📈 거래량 급증 확인", 
                        buy_amount, rsi_prev, rsi_current
                    )).await;
                    // GPT의 실제 매매 주문
                    buy_order(&client, &access_key, &secret_key, buy_amount).await;
                }
            }
            // B. 매도 로직 (트레일링 스탑 & 손절)
            if btc_amount > 0.0001 {
                // 1. 트레일링 스탑 조건: 최고 수익률이 2.0%를 넘었고, 최고점에서 1.0% 이상 하락했을 때
                let highest_profit_pct = (highest_held_price - btc_avg_price) / btc_avg_price * 100.0;
                let drop_from_highest = (highest_held_price - current_price) / highest_held_price * 100.0;
                
                let is_trailing_stop = highest_profit_pct >= 2.0 && drop_from_highest >= 1.0;
                let is_stop_loss = current_profit_pct <= -3.1;

                if is_trailing_stop {
                    alert(&format!(
                        "💰 **[트레일링 익절]** 고점 대비 하락!\n- 🚀 도달했던 최고 수익률: {:.2}%\n- 📉 현재 수익률: {:.2}%", 
                        highest_profit_pct, current_profit_pct
                    )).await;
                    sell_order(&client, &access_key, &secret_key, btc_amount).await;
                    highest_held_price = 0.0; // 매도 후 초기화
                } else if is_stop_loss {
                    alert(&format!(
                        "⚠️ **[손절]**\n- 📉 현재 수익률: {:.2}%\n- 방어선 붕괴", 
                        current_profit_pct
                    )).await;
                    sell_order(&client, &access_key, &secret_key, btc_amount).await;
                    highest_held_price = 0.0;
                }
            }
        }
        sleep(Duration::from_secs(30)).await; // 30초마다 반복
    }
}