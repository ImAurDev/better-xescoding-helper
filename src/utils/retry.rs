use std::time::Duration;

pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(5),
            multiplier: 2.0,
        }
    }
}

impl RetryPolicy {
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 1,
            ..Self::default()
        }
    }

    pub fn network() -> Self {
        Self {
            max_attempts: 4,
            initial_delay: Duration::from_millis(300),
            max_delay: Duration::from_secs(8),
            multiplier: 2.0,
        }
    }
}

pub async fn retry_async<F, Fut, T, E>(policy: &RetryPolicy, mut op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut delay = policy.initial_delay;
    let mut last_err: Option<E> = None;
    for attempt in 0..policy.max_attempts {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt + 1 >= policy.max_attempts {
                    return Err(e);
                }
                tracing::warn!(
                    "操作失败(第 {}/{} 次): {}, {}ms 后重试",
                    attempt + 1,
                    policy.max_attempts,
                    e,
                    delay.as_millis()
                );
                last_err = Some(e);
                tokio::time::sleep(delay).await;
                let next = (delay.as_millis() as f64 * policy.multiplier) as u64;
                delay = Duration::from_millis(next.min(policy.max_delay.as_millis() as u64));
            }
        }
    }
    Err(last_err.expect("retry_async called with max_attempts=0"))
}

pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 0)
}

pub fn is_retryable_reqwest_error(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_request()
}
