use fileserve::FileServer;
use tempfile::tempdir;

#[tokio::test]
async fn test_file_operations() {
    let dir = tempdir().unwrap();
    let server = FileServer::new(dir.path());

    // Write a file
    server.write_file("test.txt", b"hello world").await.unwrap();

    // Check it exists
    assert!(server.file_exists("test.txt").await);

    // Read it back
    let data = server.read_file("test.txt").await.unwrap();
    assert_eq!(data, b"hello world");
}

#[tokio::test]
async fn test_subdirectories() {
    let dir = tempdir().unwrap();
    let server = FileServer::new(dir.path());

    // Write a file in a subdirectory
    server.write_file("subdir/test.txt", b"nested").await.unwrap();

    // Verify it exists
    assert!(server.file_exists("subdir/test.txt").await);

    // Read it back
    let data = server.read_file("subdir/test.txt").await.unwrap();
    assert_eq!(data, b"nested");
}

#[tokio::test]
async fn test_list_dir() {
    let dir = tempdir().unwrap();
    let server = FileServer::new(dir.path());

    server.write_file("file1.txt", b"content1").await.unwrap();
    server.write_file("file2.txt", b"content2").await.unwrap();

    let entries = server.list_dir(".").await.unwrap();
    assert_eq!(entries.len(), 2);
}
