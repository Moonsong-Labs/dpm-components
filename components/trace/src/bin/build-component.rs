use std::{
    env, fs,
    path::PathBuf,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    match build_component() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build_component() -> Result<(), String> {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let status = Command::new("cargo")
        .args(["build", "--release", "--bin", "trace"])
        .env("CARGO_TARGET_DIR", "target")
        .current_dir(&project_root)
        .status()
        .map_err(|error| format!("failed to run cargo build: {error}"))?;

    if !status.success() {
        return Err(format!("cargo build failed with status {status}"));
    }

    let executable_name = format!("trace{}", env::consts::EXE_SUFFIX);
    let source = project_root
        .join("target")
        .join("release")
        .join(&executable_name);
    let bin_dir = project_root.join("bin");
    let destination = bin_dir.join(&executable_name);

    fs::create_dir_all(&bin_dir)
        .map_err(|error| format!("failed to create {}: {error}", bin_dir.display()))?;
    fs::copy(&source, &destination).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&destination)
            .map_err(|error| format!("failed to inspect {}: {error}", destination.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&destination, permissions).map_err(|error| {
            format!(
                "failed to mark {} as executable: {error}",
                destination.display()
            )
        })?;
    }

    println!("installed {}", destination.display());
    Ok(())
}
