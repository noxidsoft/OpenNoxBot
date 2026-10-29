use imap::{ClientBuilder, ConnectionMode};

#[test]
#[ignore = "opens a TLS connection to imap.titan.email and sends no credentials"]
fn the_titan_server_handshakes_and_greets() {
    let client = ClientBuilder::new("imap.titan.email", 993).mode(ConnectionMode::Tls).connect().expect("TLS handshake and greeting");
    drop(client);
}
