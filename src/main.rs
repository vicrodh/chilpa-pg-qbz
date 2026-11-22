use pg_qbz::runtime::process;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  process::serve(tokio::io::stdin(), tokio::io::stdout()).await;
}
