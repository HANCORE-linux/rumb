use std::process::{Command, Stdio};

#[test]
fn printing_into_a_closed_pipe_ends_quietly() {
    for arg in ["--themes", "--version", "--help"] {
        for _ in 0..20 {
            let mut child = Command::new(env!("CARGO_BIN_EXE_rumb")).arg(arg).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            drop(child.stdout.take());
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{arg}: {}", output.status);
            assert!(output.stderr.is_empty(), "{arg}: {}", String::from_utf8_lossy(&output.stderr));
        }
    }
}
