use super::platform::PlatformConnection;
use anyhow::Context;

pub struct LocalShell {}

impl PlatformConnection for LocalShell {
    async fn connect(&self) -> anyhow::Result<()> {
        match self.is_connected().await {
            true => Ok(()),
            false => Err(anyhow::anyhow!("Failed to connect to local shell")),
        }
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg("which sh")
            .output()
            .await?;
        output.status.success()
    }

    async fn run_command(&self, command: &str) -> anyhow::Result<String> {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "Command failed with status: {}",
                output.status
            ));
        }
        let stdout = String::from_utf8(&output.stdout).with_context(|| {
            format!("Output for 'sh -c {}' is incompatible with UTF-8.", command)
        })?;
        Ok(stdout)
    }
}
