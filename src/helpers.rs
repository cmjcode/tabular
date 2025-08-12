
use log::debug;

pub(crate) fn ping_host(host: &str) -> bool {
    if host.contains("/data") {
        // check if file is exists from path in string host
        std::path::Path::new(host).exists()
    } else {
        #[cfg(target_os = "macos")]
        let output = std::process::Command::new("ping")
            .arg("-c")
            .arg("1")
            .arg("-W")
            .arg("2000") // timeout 2000 ms (2 detik) pada macOS           
            .arg(host)
            .output();

        #[cfg(target_os = "linux")]
        let output = std::process::Command::new("ping")
            .arg("-c")
            .arg("1")
            .arg("-W")
            .arg("2") // timeout 2 detik            
            .arg(host)
            .output();

        #[cfg(target_os = "windows")]
        let output = std::process::Command::new("ping")
            .arg("-n")
            .arg("1")
            .arg("-w")
            .arg("2000") // timeout 2000 ms (2 detik) untuk Windows            
            .arg(host)
            .output();

        debug!("Ping output: {:?}", output);

        match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }

}