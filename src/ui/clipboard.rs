use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

/// None means an OS clipboard helper completed; Some is an OSC52 request for the UI writer.
pub async fn copy(text: String) -> Result<Option<String>> {
    let remote =
        std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
    if !remote {
        #[cfg(target_os = "macos")]
        let helpers: Vec<(&str, &[&str])> = vec![("/usr/bin/pbcopy", &[])];
        #[cfg(not(target_os = "macos"))]
        let helpers: Vec<(&str, &[&str])> = {
            let mut commands = vec![];
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                commands.push(("wl-copy", &[][..]));
            }
            if std::env::var_os("DISPLAY").is_some() {
                commands.push(("xclip", &["-selection", "clipboard"][..]));
                commands.push(("xsel", &["--clipboard", "--input"][..]));
            }
            commands
        };
        for (program, args) in helpers {
            if write_helper(program, args, &text).await.is_ok() {
                return Ok(None);
            }
        }
    }
    osc52(&text, std::env::var_os("TMUX").is_some()).map(Some)
}

async fn write_helper(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .context("Clipboard helper has no stdin")?;
    tokio::time::timeout(Duration::from_secs(2), async {
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);
        ensure!(child.wait().await?.success(), "Clipboard helper failed");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Clipboard helper timed out")??;
    Ok(())
}
fn osc52(text: &str, tmux: bool) -> Result<String> {
    ensure!(
        text.len() <= 100_000,
        "Terminal clipboard transfer is limited to 100 KB; select less text or install a clipboard helper"
    );
    let sequence = format!("\x1b]52;c;{}\x07", STANDARD.encode(text));
    Ok(if tmux {
        format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
    } else {
        sequence
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn osc52_encodes_untrusted_text_and_tmux_envelope() {
        let text = "中\n$(touch /tmp/never)\x1b[2J";
        assert_eq!(
            osc52(text, false).unwrap(),
            format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
        );
        assert!(
            osc52(text, true)
                .unwrap()
                .starts_with("\x1bPtmux;\x1b\x1b]52;")
        );
        assert!(osc52(&"x".repeat(100001), false).is_err());
    }
    #[tokio::test]
    async fn clipboard_helper_receives_literal_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("copy");
        let output = dir.path().join("result");
        std::fs::write(&script, "#!/bin/sh\ncat > \"$1\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let value = "a\n中🙂'$(whoami)`id`";
        write_helper(script.to_str().unwrap(), &[output.to_str().unwrap()], value)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(output).unwrap(), value);
    }
}
