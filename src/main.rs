use rustdiscordbot::http::HttpParsed;
use serde::Deserialize;
use openssl::ssl::SslConnector;

#[derive(Deserialize)]
struct DiscordApiGatewayResponse {
    url: String
}

struct Url {
    protocol: String,
    host: String,
    path: String,
}
impl Url {
    fn split_url(url: &str) -> Option<Self> {
        let (protocol, url) = url.split_once("://")?;
        let (host, path) = url.split_once("/")
            .unwrap_or_else(|| (url, "/"));
        Some(Url {
            protocol: protocol.to_owned(),
            host: host.to_owned(),
            path: path.to_owned(),
        })
    }
}

fn ssl_connector() -> SslConnector {
    use openssl::x509;
    use openssl::ssl::{SslMethod, SslConnector};
    use std::include_bytes;

    let x509 = {
        let cacert = include_bytes!(env!("CACERTS_PATH"));
        x509::X509::from_pem(cacert).unwrap()
    };

    let x509_store = {
        let mut builder = x509::store::X509StoreBuilder::new().unwrap();
        builder.add_cert(x509).unwrap();
        builder.build()
    };

    let mut builder = SslConnector::builder(SslMethod::tls()).unwrap();
    builder.set_cert_store(x509_store);
    builder.build()
}

struct WebSocketSecKey {
    ws_key: String,
    ws_accept: String,
}
fn websocket_sec_key() -> WebSocketSecKey {
    use rand::prelude::Rng;
    use base64::prelude::{BASE64_STANDARD, Engine};
    let nonce = [rand::rng().random::<u8>(), rand::rng().random::<u8>()];
    let ws_key = BASE64_STANDARD.encode(&nonce);
    let key_concat = ws_key.clone() + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = openssl::hash::Hasher::new(openssl::hash::MessageDigest::sha1()).unwrap();
    hasher.update(&key_concat.into_bytes()).unwrap();
    let output = hasher.finish().unwrap();
    let ws_accept = BASE64_STANDARD.encode(output);

    WebSocketSecKey {
        ws_key,
        ws_accept,
    }
}

fn websocket_validate_headers(ws_accept: String, HttpParsed { code, msg: _, headers, body: _ }: HttpParsed) -> Result<(), String> {
    if code.ok_or("Missing HTTP response code")? != 101 {
        Err(format!("Invalid websocket response code: {}", code.unwrap()))
    } else if headers.get("upgrade").ok_or("Missing HTTP Upgrade header")? != "websocket" {
        Err(format!("HTTP Upgrade header is not \"websocket\": {}", headers.get("upgrade").unwrap()))
    } else if headers.get("connection").ok_or("Missing HTTP Connection header")? != "upgrade" {
        Err(format!("HTTP Connection header is not \"upgrade\": {}", headers.get("connection").unwrap()))
    } else if *headers.get("sec-websocket-accept").ok_or("Missing WebSocket sec accept header")? != ws_accept {
        Err(format!("HTTP Sec-WebSocket-Accept header is not \"{}\": {}", ws_accept, headers.get("sec-websocket-accept").unwrap()))
    } else if headers.get("sec-websocket-extensions").is_some() {
        Err(format!("Unexpected Sec-WebSocket-Extensions header: {}", headers.get("sec-websocket-extensions").unwrap()))
    } else if headers.get("sec-websocket-protocol").is_some() {
        Err(format!("Unexpected Sec-WebSocket-Protocol header: {}", headers.get("sec-websocket-protocol").unwrap()))
    } else {
        Ok(())
    }
}

fn main() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let connector = ssl_connector();
    let response: HttpParsed = {
        let stream = TcpStream::connect("discord.com:443").unwrap();
        let mut stream = connector.connect("discord.com", stream).unwrap();

        stream.write_all(b"GET /api/gateway HTTP/1.1\r\nHost: discord.com\r\nUser-Agent: test/1.0\r\nAccept: */*\r\n\r\n").unwrap();
        HttpParsed::read_to_end::<_, 8192>(&mut stream)
    };
    let DiscordApiGatewayResponse { url: gateway_url } = serde_json::from_str(&response.body.unwrap()).unwrap();
    let Url { protocol, host: gateway_host, path: gateway_path } = Url::split_url(&gateway_url).unwrap();
    if protocol != "wss" {
        panic!("Discord gateway is the wrong protocol!");
    }
    println!("Connecting to Discord Websocket gateway: {}", gateway_host);

    let stream = TcpStream::connect(format!("{}:443", gateway_host)).unwrap();
    let mut stream = connector.connect(&gateway_host, stream).unwrap();
    let WebSocketSecKey { ws_key, ws_accept } = websocket_sec_key();

    let ws_request = format!("GET {} HTTP/1.1\r\n\
                          Host: {}\r\n\
                          Upgrade: websocket\r\n\
                          Connection: Upgrade\r\n\
                          Sec-WebSocket-Key: {}\r\n\
                          Sec-WebSocket-Version: 13\r\n\r\n", gateway_path, gateway_host, ws_key);
    stream.write_all(&ws_request.into_bytes()).unwrap();
    let headers = HttpParsed::read_headers::<_, 8192>(&mut stream);
    websocket_validate_headers(ws_accept, headers).unwrap();
    println!("WebSocket headers validated");

    println!("Reading body");
    let mut buf = [0u8; 8192];
    'outer: loop {
        match stream.read(&mut buf) {
            Ok(n) => {
                for b in &buf[0..n] {
                    print!("{:x}, ", b);
                }
                println!("");
            },
            Err(err) => match err.kind() {
                std::io::ErrorKind::UnexpectedEof => {
                    println!("EOF");
                    break 'outer;
                },
                _ => panic!("{}", err),
            },
        }
        buf.fill(0);
    }
}

