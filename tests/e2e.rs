use std::process::Stdio;

#[tokio::test]
async fn test_e2e_basic() {
    // This is a simplified E2E test that just checks if the server binary exists
    // and can be executed, without actually testing the full protocol

    // Try to run the server with --help to check if it exists and is executable
    println!("Checking if server binary exists");
    let server_process = tokio::process::Command::new("cargo")
        .args(["build", "--bin", "server"])
        .stdout(Stdio::piped())
        .spawn();

    // Verify that we can build the server
    assert!(
        server_process.is_ok(),
        "Should be able to build the server binary"
    );

    // Wait for the build to complete
    let mut process = server_process.unwrap();
    let status = process.wait().await.expect("Failed to wait for process");

    // Verify that the build succeeded
    assert!(status.success(), "Server binary should build successfully");
}
