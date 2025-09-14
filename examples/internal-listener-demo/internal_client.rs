use std::env;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        println!("Usage: {} <test_type>", args[0]);
        println!("Test types: gateway, admin, health");
        return;
    }

    match args[1].as_str() {
        "gateway" => test_gateway(),
        "admin" => test_admin(),
        "health" => test_health(),
        _ => println!("Invalid test type. Use: gateway, admin, health"),
    }
}

fn test_gateway() {
    println!("Testing External Gateway");
    
    let endpoints = vec![
        ("localhost:10000", "/", "Main Gateway"),
        ("localhost:10000", "/service-a", "Service A"),
    ];
    
    for (address, path, description) in endpoints {
        println!("Testing {}: http://{}{}", description, address, path);
        match test_http_endpoint(address, path) {
            Ok(response) => println!("  ✅ {}", response.trim()),
            Err(e) => println!("  ❌ {}", e),
        }
    }
}

fn test_admin() {
    println!("Testing Admin Interface");
    
    let endpoints = vec![
        ("localhost:9901", "/stats", "Statistics"),
        ("localhost:9901", "/listeners", "Listeners"),
        ("localhost:9901", "/clusters", "Clusters"),
    ];
    
    for (address, path, description) in endpoints {
        println!("Testing {}: http://{}{}", description, address, path);
        match test_http_endpoint(address, path) {
            Ok(response) => {
                println!("  ✅ {} lines received", response.lines().count());
                for line in response.lines().take(2) {
                    if !line.trim().is_empty() {
                        println!("  {}", line.trim());
                    }
                }
            }
            Err(e) => println!("  ❌ {}", e),
        }
    }
}

fn test_health() {
    println!("Testing Health Metrics");
    
    match test_http_endpoint("localhost:9901", "/stats") {
        Ok(response) => {
            let internal_stats: Vec<&str> = response
                .lines()
                .filter(|line| line.contains("internal") || line.contains("listener"))
                .take(5)
                .collect();
                
            if internal_stats.is_empty() {
                println!("  ⚠️  No internal listener stats found");
            } else {
                for stat in internal_stats {
                    println!("  📊 {}", stat.trim());
                }
            }
        }
        Err(e) => println!("  ❌ {}", e),
    }
}

fn test_http_endpoint(address: &str, path: &str) -> Result<String, String> {
    let addr = address.to_socket_addrs()
        .map_err(|e| format!("Failed to resolve {}: {}", address, e))?
        .next()
        .ok_or_else(|| format!("No addresses found for {}", address))?;
    
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("Connection failed: {}", e))?;
    
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, address.split(':').next().unwrap_or("localhost")
    );
    
    stream.write_all(request.as_bytes())
        .map_err(|e| format!("Failed to send request: {}", e))?;
    
    let mut response = String::new();
    stream.read_to_string(&mut response)
        .map_err(|e| format!("Failed to read response: {}", e))?;
    
    if let Some(body_start) = response.find("\r\n\r\n") {
        let (headers, body) = response.split_at(body_start + 4);
        
        if headers.contains("200 OK") {
            Ok(body.to_string())
        } else if headers.contains("404") {
            Err("Endpoint not found (404)".to_string())
        } else {
            Err(format!("HTTP error: {}", headers.lines().next().unwrap_or("Unknown")))
        }
    } else {
        Err("Invalid HTTP response".to_string())
    }
}
