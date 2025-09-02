use std::env;
use std::io::Write;
use std::net::{TcpStream, IpAddr};

fn construct_tlv_packet(ip: &str, port: u16) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Parse IP address
    let ip_addr: IpAddr = ip.parse()?;
    let (ip_bytes, content_len) = match ip_addr {
        IpAddr::V4(ipv4) => (ipv4.octets().to_vec(), 6u32), // 4 bytes IP + 2 bytes port
        IpAddr::V6(ipv6) => (ipv6.octets().to_vec(), 18u32), // 16 bytes IP + 2 bytes port
    };

    // Pack port as big-endian
    let port_bytes = port.to_be_bytes();

    // TLV structure:
    // Type: 0x01 (service address)
    // Length: 4 bytes (big-endian)
    // Content: IP + Port
    // End: 0xfe

    let content = [ip_bytes, port_bytes.to_vec()].concat();
    let length = content_len.to_be_bytes();

    let mut tlv_packet = vec![0x01]; // Type
    tlv_packet.extend_from_slice(&length); // Length
    tlv_packet.extend_from_slice(&content); // Content
    
    // End marker: Type 0xfe, Length 0
    tlv_packet.push(0xfe); // End type
    tlv_packet.extend_from_slice(&0u32.to_be_bytes()); // End length (0)

    Ok(tlv_packet)
}

fn send_tlv_packet(ip: &str, port: u16, listener_host: &str, listener_port: u16) -> Result<(), Box<dyn std::error::Error>> {
    // Create TLV packet
    let tlv_data = construct_tlv_packet(ip, port)?;
    println!("Constructed TLV packet: {:02x?}", tlv_data);

    // Create HTTP request to follow
    let http_request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";

    // Combine TLV + HTTP
    let mut packet = tlv_data;
    packet.extend_from_slice(http_request);

    // Send to listener
    let mut stream = TcpStream::connect((listener_host, listener_port))?;
    stream.write_all(&packet)?;

    println!("✅ Sent TLV packet with original destination {}:{}", ip, port);
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 3 {
        eprintln!("Usage: {} <ip> <port>", args[0]);
        std::process::exit(1);
    }

    let ip = &args[1];
    let port: u16 = args[2].parse()?;

    match send_tlv_packet(ip, port, "127.0.0.1", 10000) {
        Ok(()) => {
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("❌ Failed to send TLV packet: {}", e);
            std::process::exit(1);
        }
    }
}
