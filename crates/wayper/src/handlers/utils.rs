use rand::seq::SliceRandom;
use walkdir::WalkDir;

pub fn run_command(command: String, img_path: std::path::PathBuf) {
    let mut command = shlex::Shlex::new(&command).collect::<Vec<_>>();

    // rudimentary substitution that I can't figure out how to do in place
    for arg in command.iter_mut() {
        if arg == "{image}" {
            arg.clear();
            arg.push_str(&img_path.display().to_string());
        }
    }

    tracing::info!("running command {}", command.join(" "));

    // let chains wooooo
    if let Some((command, args)) = command.split_first()
        && let Ok(child) = std::process::Command::new(command).args(args).output()
    {
        if !child.stdout.is_empty() {
            tracing::info!(
                "command stdout: {}",
                String::from_utf8(child.stdout).unwrap_or_default()
            );
        }

        if !child.stderr.is_empty() {
            tracing::info!(
                "command stderr: {}",
                String::from_utf8(child.stderr).unwrap_or_default()
            );
        }
        tracing::info!("command exited with code {:?}", child.status.code());
    } else {
        tracing::error!("command run error, check if the command exists and is correct");
    }
}

/// Get a list of images from a config
pub fn get_img_list(
    output_config: Option<&wayper_lib::config::OutputConfig>,
) -> Vec<std::path::PathBuf> {
    if let Some(output_config) = output_config {
        if output_config.path.is_file() {
            vec![output_config.path.clone()]
        } else if output_config.path.is_dir() {
            let mut files = WalkDir::new(&output_config.path)
                .into_iter()
                .filter(|e| {
                    mime_guess::from_path(e.as_ref().unwrap().path())
                        .iter()
                        .any(|ev| ev.type_() == "image")
                })
                .map(|e| e.unwrap().path().to_owned())
                .collect::<Vec<_>>();

            let mut rng = rand::rng();
            files.shuffle(&mut rng);
            tracing::debug!("{:?}", &files);
            files
        } else {
            vec![]
        }
    } else {
        vec![]
    }
}
