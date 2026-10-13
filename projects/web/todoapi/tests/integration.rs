use todoapi::{Store, CreateTodoReq, UpdateTodoReq};

#[tokio::test]
async fn test_create_and_list() {
    let store = Store::new();

    let todo = store.create(CreateTodoReq {
        title: "test".to_string(),
    }).await;

    assert_eq!(todo.title, "test");
    assert!(!todo.done);
    assert_eq!(todo.id, 1);

    let all = store.list().await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].title, "test");
}

#[tokio::test]
async fn test_update() {
    let store = Store::new();

    let todo = store.create(CreateTodoReq {
        title: "original".to_string(),
    }).await;

    let updated = store.update(todo.id, UpdateTodoReq {
        title: Some("updated".to_string()),
        done: Some(true),
    }).await;

    assert!(updated.is_some());
    let todo = updated.unwrap();
    assert_eq!(todo.title, "updated");
    assert!(todo.done);
}

#[tokio::test]
async fn test_delete() {
    let store = Store::new();

    let todo = store.create(CreateTodoReq {
        title: "delete me".to_string(),
    }).await;

    assert!(store.delete(todo.id).await);
    assert!(store.get(todo.id).await.is_none());
}

#[tokio::test]
async fn test_get() {
    let store = Store::new();

    let todo = store.create(CreateTodoReq {
        title: "get me".to_string(),
    }).await;

    let retrieved = store.get(todo.id).await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().title, "get me");
}
