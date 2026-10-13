use metricsd::MetricsCollector;

#[tokio::test]
async fn test_record_and_get() {
    let collector = MetricsCollector::new();

    collector.record("cpu".to_string(), 75.5).await;

    let value = collector.get("cpu").await;
    assert_eq!(value, Some(75.5));
}

#[tokio::test]
async fn test_get_all() {
    let collector = MetricsCollector::new();

    collector.record("mem".to_string(), 512.0).await;
    collector.record("cpu".to_string(), 50.0).await;

    let all = collector.get_all().await;
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_broadcast_subscribe() {
    let collector = MetricsCollector::new();
    let mut rx = collector.subscribe();

    collector.record("test".to_string(), 42.0).await;

    let metric = rx.recv().await;
    assert!(metric.is_ok());
    let metric = metric.unwrap();
    assert_eq!(metric.name, "test");
    assert_eq!(metric.value, 42.0);
}
