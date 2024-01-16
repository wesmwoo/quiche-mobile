
use quiche_apps::mobman::*;

#[tokio::main]
async fn main() {
    mobman_unix().await;
}