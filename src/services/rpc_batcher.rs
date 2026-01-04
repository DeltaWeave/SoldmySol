/// RPC Request Batching
///
/// Enables parallel execution of multiple RPC requests to reduce latency.
/// Instead of making sequential calls, batch related calls together.
///
/// Example:
/// - Get multiple token prices in parallel
/// - Check multiple account balances at once
/// - Fetch multiple transaction statuses concurrently

use anyhow::Result;
use futures::future::join_all;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Batch executor for running multiple async tasks in parallel
pub struct RpcBatcher {
    max_concurrent: usize,
}

impl RpcBatcher {
    pub fn new(max_concurrent: usize) -> Self {
        Self { max_concurrent }
    }

    /// Execute multiple futures in parallel with concurrency limit
    pub async fn batch_execute<T, F, Fut>(&self, tasks: Vec<F>) -> Vec<Result<T>>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let mut results = Vec::with_capacity(tasks.len());
        let mut handles: Vec<JoinHandle<Result<T>>> = Vec::new();

        for task in tasks {
            // Spawn task
            let handle = tokio::spawn(async move { task().await });
            handles.push(handle);

            // If we've reached max concurrent, wait for some to complete
            if handles.len() >= self.max_concurrent {
                // Wait for the oldest task
                if !handles.is_empty() {
                let handle = handles.remove(0);
                    match handle.await {
                        Ok(result) => results.push(result),
                        Err(e) => results.push(Err(anyhow::anyhow!("Task panicked: {}", e))),
                    }
                }
            }
        }

        // Wait for remaining tasks
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(anyhow::anyhow!("Task panicked: {}", e))),
            }
        }

        results
    }

    /// Execute multiple futures in parallel (no concurrency limit)
    pub async fn batch_execute_all<T, F, Fut>(tasks: Vec<F>) -> Vec<Result<T>>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let futures: Vec<_> = tasks.into_iter().map(|task| tokio::spawn(async move { task().await })).collect();

        let results = join_all(futures).await;

        results
            .into_iter()
            .map(|res| match res {
                Ok(result) => result,
                Err(e) => Err(anyhow::anyhow!("Task panicked: {}", e)),
            })
            .collect()
    }
}

impl Default for RpcBatcher {
    fn default() -> Self {
        Self::new(10) // Default to 10 concurrent requests
    }
}

/// Helper functions for common batching patterns

/// Batch multiple token price fetches
pub async fn batch_get_prices<F, Fut>(
    token_addresses: Vec<String>,
    price_fetcher: Arc<F>,
) -> Vec<Result<(String, f64)>>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<f64>> + Send + 'static,
{
    let tasks: Vec<_> = token_addresses
        .into_iter()
        .map(|addr| {
            let fetcher = price_fetcher.clone();
            let address = addr.clone();
            move || async move {
                let price = fetcher(address.clone()).await?;
                Ok((address, price))
            }
        })
        .collect();

    RpcBatcher::batch_execute_all(tasks).await
}

/// Batch multiple account balance fetches
pub async fn batch_get_balances<F, Fut>(
    addresses: Vec<String>,
    balance_fetcher: Arc<F>,
) -> Vec<Result<(String, f64)>>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<f64>> + Send + 'static,
{
    let tasks: Vec<_> = addresses
        .into_iter()
        .map(|addr| {
            let fetcher = balance_fetcher.clone();
            let address = addr.clone();
            move || async move {
                let balance = fetcher(address.clone()).await?;
                Ok((address, balance))
            }
        })
        .collect();

    RpcBatcher::batch_execute_all(tasks).await
}

/// Batch execution with custom handlers
pub struct BatchRequest<T> {
    requests: Vec<Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send>> + Send>>,
}

impl<T: Send + 'static> BatchRequest<T> {
    pub fn new() -> Self {
        Self {
            requests: Vec::new(),
        }
    }

    pub fn add<F, Fut>(&mut self, request: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        self.requests.push(Box::new(move || Box::pin(request())));
    }

    pub async fn execute(self) -> Vec<Result<T>> {
        let futures: Vec<_> = self
            .requests
            .into_iter()
            .map(|req| tokio::spawn(async move { req().await }))
            .collect();

        let results = join_all(futures).await;

        results
            .into_iter()
            .map(|res| match res {
                Ok(result) => result,
                Err(e) => Err(anyhow::anyhow!("Task panicked: {}", e)),
            })
            .collect()
    }
}

impl<T: Send + 'static> Default for BatchRequest<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_batch_execute() {
        let batcher = RpcBatcher::new(3);

        let tasks: Vec<_> = (0..5)
            .map(|i| move || async move {
                sleep(Duration::from_millis(10)).await;
                Ok::<i32, anyhow::Error>(i)
            })
            .collect();

        let results = batcher.batch_execute(tasks).await;

        assert_eq!(results.len(), 5);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result.as_ref().unwrap(), &(i as i32));
        }
    }

    #[tokio::test]
    async fn test_batch_execute_all() {
        let tasks: Vec<_> = (0..5)
            .map(|i| move || async move {
                sleep(Duration::from_millis(10)).await;
                Ok::<i32, anyhow::Error>(i * 2)
            })
            .collect();

        let results = RpcBatcher::batch_execute_all(tasks).await;

        assert_eq!(results.len(), 5);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result.as_ref().unwrap(), &((i * 2) as i32));
        }
    }

    #[tokio::test]
    async fn test_batch_get_prices() {
        let price_fetcher = Arc::new(|token: String| async move {
            sleep(Duration::from_millis(5)).await;
            // Simulate price fetching
            Ok::<f64, anyhow::Error>(100.0 + token.len() as f64)
        });

        let tokens = vec![
            "token1".to_string(),
            "token2".to_string(),
            "token3".to_string(),
        ];

        let results = batch_get_prices(tokens, price_fetcher).await;

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert_eq!(results[0].as_ref().unwrap().1, 106.0); // 100 + 6 (length of "token1")
    }

    #[tokio::test]
    async fn test_batch_request() {
        let mut batch = BatchRequest::new();

        for i in 0..3 {
            batch.add(move || async move {
                sleep(Duration::from_millis(5)).await;
                Ok::<i32, anyhow::Error>(i * 10)
            });
        }

        let results = batch.execute().await;

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap(), &0);
        assert_eq!(results[1].as_ref().unwrap(), &10);
        assert_eq!(results[2].as_ref().unwrap(), &20);
    }

    #[tokio::test]
    async fn test_error_handling() {
        let tasks: Vec<_> = vec![
            move || async move { Ok::<i32, anyhow::Error>(1) },
            move || async move { Err(anyhow::anyhow!("Test error")) },
            move || async move { Ok::<i32, anyhow::Error>(3) },
        ];

        let results = RpcBatcher::batch_execute_all(tasks).await;

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }
}
