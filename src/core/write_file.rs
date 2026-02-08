use std::{fs::File, io::{ErrorKind, Write}, process::{Command, Stdio}};

pub fn write_file(path: &str, content: &str) -> Result<(), String> {
    match File::create(path) {
        Ok(mut f) => {
            let _ = match f.write(&content.as_bytes()) {
                Ok(_) => return Ok(()),
                Err(err) => return Err(err.to_string()),
            };
        },
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            let mut child = match  Command::new("pkexec")
                .arg("tee")
                .arg(path)
                .stdin(std::process::Stdio::piped())
                .stdout(Stdio::null())
                .spawn() {
                    Ok(p) => p,
                    Err(e) => return Err(e.to_string()),
                };

                if let Some(stdin) = child.stdin.as_mut() {
                    match stdin.write_all(content
                         .as_bytes()) {
                        Ok(_) => return Ok(()),
                        Err(e) => return Err(e.to_string()),
                    }
                } else {
                    return Err(String::from("Impossible to write file"))
                }
        },
        Err(e) => return Err(e.to_string())
    }
}
