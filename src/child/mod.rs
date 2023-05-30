use std::io::BufRead;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use anyhow::anyhow;
use nix::unistd::{setsid, Pid};

pub trait LinuxChild {
    fn get_session_id(&self) -> nix::Result<nix::unistd::Pid>;
    fn kill_process_group<T: Into<Option<nix::sys::signal::Signal>>>(
        &self,
        signal: T,
    ) -> nix::Result<()>;
}

impl LinuxChild for Child {
    fn get_session_id(&self) -> nix::Result<nix::unistd::Pid> {
        nix::unistd::getsid(Some(Pid::from_raw(self.id() as i32)))
    }

    fn kill_process_group<T>(&self, signal: T) -> nix::Result<()>
    where
        T: Into<Option<nix::sys::signal::Signal>>,
    {
        nix::sys::signal::killpg(self.get_session_id()?, signal)
    }
}

pub fn spawn_child(cmd: &str) -> anyhow::Result<Child> {
    let cmd_words = shell_words::split(cmd)?;

    let mut child = unsafe {
        // Setting a pre_exec hook is unsafe here, because the closure will run
        // in a weird environment between fork an exec.
        // All other operations are safe
        Command::new(&cmd_words[0])
            .args(&cmd_words[1..])
            .pre_exec(|| {
                setsid().expect("Failed to set a new session id for child process");
                Ok(())
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
    };

    let child_stdout = child
        .stdout
        .take()
        .ok_or(anyhow!("Couldn't get child stdout"))?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or(anyhow!("Couldn't get child stderr"))?;

    tokio::spawn(async {
        let stdout_lines = std::io::BufReader::new(child_stdout).lines();
        for line in stdout_lines {
            match line {
                Ok(l) => println!("{}", l),
                Err(_) => break,
            }
        }
    });

    tokio::spawn(async {
        let stderr_lines = std::io::BufReader::new(child_stderr).lines();
        for line in stderr_lines {
            match line {
                Ok(l) => eprintln!("{}", l),
                Err(_) => break,
            }
        }
    });

    Ok(child)
}
