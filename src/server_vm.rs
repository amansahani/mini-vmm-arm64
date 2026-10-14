use kasou::VmConfigBuilder;
use std::io::copy;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;

/// Demonstrates creating and configuring a full production-ready microVM on macOS
/// to host a Web Server (listening on port 80) and periodic cron jobs.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!("  Production MicroVM: Docker-like Web Server & Cron Host   ");
    println!("============================================================");

    // 1. Assets definition
    let assets_dir = PathBuf::from("./assets");
    let kernel_path = assets_dir.join("Image");
    let initrd_path = assets_dir.join("initramfs-virt");
    let rootfs_path = assets_dir.join("rootfs.img");
    let console_log = PathBuf::from("/tmp/microvm_console.log");
    let web_root = PathBuf::from("./www");

    println!("[1/4] Configuring VM specification via Kasou / Virtualization.framework:");
    println!("      - 2 vCPUs, 1024 MiB RAM");
    println!("      - Kernel: {}", kernel_path.display());
    println!("      - Rootfs: {}", rootfs_path.display());
    println!("      - Network: VirtIO NAT (macOS DHCP subnet 192.168.64.0/24)");
    println!("      - Live Shared Directory (VirtioFS): host '{}' -> guest '/var/www'", web_root.display());
    println!("      - Serial Console: {}", console_log.display());

    // 2. Build the VM Configuration
    let mut builder = VmConfigBuilder::new("web-cron-microvm")
        .cpus(2)
        .memory_mib(1024)
        .cmdline("console=hvc0 root=/dev/vda rw ip=dhcp")
        .nat_network()
        .mac("c2:ab:66:91:3d:64")
        .serial_file(console_log.clone());

    if kernel_path.exists() {
        builder = builder.boot(kernel_path);
    }
    if initrd_path.exists() {
        builder = builder.initrd(initrd_path);
    }
    if rootfs_path.exists() {
        builder = builder.disk(rootfs_path);
    }
    if web_root.exists() {
        builder = builder.shared_dir("app_share", web_root, false);
    }

    // 3. Port Forwarder demonstration
    println!("\n[2/4] Initializing TCP Port Forwarder (Reverse Proxy):");
    println!("      Host: http://127.0.0.1:8080 <---> MicroVM Guest: 192.168.64.3:80");

    let _proxy_handle = thread::spawn(|| {
        start_tcp_proxy(8080, "192.168.64.3", 80);
    });

    println!("\n[3/4] MicroVM In-Guest Architecture:");
    println!("      +-- [PID 1: Init / Systemd] --------------------------------------+");
    println!("      |  1. VirtioFS Mount: `mount -t virtiofs app_share /var/www`      |");
    println!("      |  2. HTTP Web Server: `nginx -g 'daemon off;'` on port 80        |");
    println!("      |  3. Cron Daemon: `crond -b -l 2` executing periodic tasks       |");
    println!("      +-----------------------------------------------------------------+");

    println!("\n[4/4] Validating and starting MicroVM...");
    let config = builder.build()?;
    println!("[+] VM Config validation passed! Starting virtual machine...");
    println!("    VM ID: {}", config.id);
    println!("    vCPUs: {}", config.cpus);
    println!("    RAM: {} MiB", config.memory_mib);

    let vm_handle = kasou::VmHandle::create(config)?;
    vm_handle.start()?;
    println!("\n============================================================");
    println!("  [+] MicroVM is now RUNNING live on macOS Apple Silicon!");
    println!("  [+] Console output being written to /tmp/microvm_console.log");
    println!("  [+] TCP Proxy active on http://127.0.0.1:8080");
    println!("============================================================");

    println!("[*] MicroVM is live! Monitoring status & console (Press Ctrl+C to stop)...");
    println!("    Try making a request from your Mac:  curl http://localhost:8080/\n");

    // Loop indefinitely to keep the VM alive
    let mut last_size = 0;
    loop {
        thread::sleep(std::time::Duration::from_millis(1500));
        if let Ok(metadata) = std::fs::metadata(&console_log) {
            let current_size = metadata.len();
            if current_size > last_size {
                if let Ok(content) = std::fs::read_to_string(&console_log) {
                    let lines: Vec<&str> = content.lines().collect();
                    if let Some(last_line) = lines.last() {
                        if last_line.contains("[Guest Cron") {
                            println!("    {}", last_line);
                        }
                    }
                }
                last_size = current_size;
            }
        }
    }
    #[allow(unreachable_code)]
    Ok(())
}

/// TCP Port Forwarder: forwards connections from host port to guest IP:port
fn start_tcp_proxy(host_port: u16, guest_ip: &str, guest_port: u16) {
    let host_bind = format!("127.0.0.1:{}", host_port);
    let target_addr = format!("{}:{}", guest_ip, guest_port);

    if let Ok(listener) = TcpListener::bind(&host_bind) {
        println!("[Proxy] Listening on http://{}", host_bind);
        for client_stream in listener.incoming().flatten() {
            let target = target_addr.clone();
            thread::spawn(move || {
                if let Ok(server_stream) = TcpStream::connect(&target) {
                    let mut c_read = client_stream.try_clone().unwrap();
                    let mut c_write = client_stream;
                    let mut s_read = server_stream.try_clone().unwrap();
                    let mut s_write = server_stream;

                    let t1 = thread::spawn(move || copy(&mut c_read, &mut s_write));
                    let t2 = thread::spawn(move || copy(&mut s_read, &mut c_write));
                    let _ = t1.join();
                    let _ = t2.join();
                }
            });
        }
    }
}
