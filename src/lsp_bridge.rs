use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
pub struct ClangdBridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}
impl ClangdBridge {
    pub fn start(project: &Path) -> io::Result<Self> {
        let jobs = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let mut child = Command::new("clangd")
            .current_dir(project)
            .args(["--background-index", "--pch-storage=memory"])
            .arg(format!("-j={jobs}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("clangd stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("clangd stdout unavailable"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }
    pub fn send(&mut self, json: &str) -> io::Result<()> {
        write!(
            self.stdin,
            "Content-Length: {}\r\n\r\n{}",
            json.as_bytes().len(),
            json
        )?;
        self.stdin.flush()
    }
    pub fn receive(&mut self) -> io::Result<String> {
        let mut length = None;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "clangd exited",
                ));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                );
            }
        }
        let n = length
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
        if n > 32 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized LSP message",
            ));
        }
        let mut body = vec![0; n];
        self.stdout.read_exact(&mut body)?;
        String::from_utf8(body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for ClangdBridge {
    fn drop(&mut self) {
        self.stop();
    }
}
pub fn generate_compilation_database(
    cli: &Path,
    sketch: &Path,
    fqbn: &str,
    build_path: &Path,
) -> io::Result<std::process::Output> {
    std::fs::create_dir_all(build_path)?;
    Command::new(cli)
        .args([
            "compile",
            "--only-compilation-database",
            "--fqbn",
            fqbn,
            "--build-path",
        ])
        .arg(build_path)
        .arg(sketch)
        .output()
}
pub fn write_frame(out: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(&body)?;
    out.flush()
}
