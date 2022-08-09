use std::{
    collections::HashMap,
    env,
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), Error> {
    // Create iterator over all the files in the XDG_DATA_DIRS
    // XDG compliancy is cool
    let global_paths: &mut Vec<Result<fs::DirEntry, io::Error>> = match env::var("XDG_DATA_DIRS") {
        Ok(data_dirs) => {
            // The vec for all the DirEntry objects
            let mut paths = Vec::new();
            // Parse the XDG_DATA_DIRS variable and list files of all the paths
            for dir in data_dirs.split(":") {
                match fs::read_dir(format!("{}/applications/", dir)) {
                    Ok(dir) => {
                        paths.extend(dir);
                    }
                    Err(why) => {
                        eprintln!("Error reading directory {}: {}", dir, why);
                    }
                }
            }
            // Make sure the list of paths isn't empty
            if paths.is_empty() {
                return Err(Error::from("No valid desktop file dirs found!".to_string()));
            }

            // Return it
            Box::leak(Box::new(paths))
        }
        Err(_) => Box::leak(Box::new(fs::read_dir("/usr/share/applications")?.collect())),
    };
    let mut handles = Vec::new();

    // Global map of values
    let map: Arc<Mutex<HashMap<String, (String, PathBuf)>>> = Arc::new(Mutex::new(HashMap::new()));

    for chunk in global_paths.chunks(global_paths.len() / 6) {
        // Clone the global hashmap
        let map = Arc::clone(&map);
        handles.push(thread::spawn(move || {
            let mut temp_map = HashMap::new();
            for entry in chunk {
                let entry = match entry.as_ref() {
                    Ok(entry) => entry,
                    Err(why) => {
                        eprintln!("Error reading directory entry: {}", why);
                        continue;
                    }
                };
                match parse_desktop_file(entry) {
                    Some((name, path)) => {
                        temp_map.insert(
                            path.file_name().unwrap().to_string_lossy().to_string(),
                            (name, path),
                        );
                    }
                    None => (),
                }
            }
            map.lock().unwrap().extend(temp_map.into_iter());
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Get the lock as we no longer need to worry about threads
    let mut map = map.lock().unwrap();

    // Go through user directory desktop files for overrides
    let user_path = match env::var("XDG_DATA_HOME") {
        Ok(data_home) => {
            format!("{}/applications/", data_home)
        }
        Err(_) => {
            format!("{}/.local/share/applications/", env::var("HOME").expect("Unable to determine home directory!"))
        }
    };

    for entry in fs::read_dir(&user_path)? {
        let entry = match entry.as_ref() {
            Ok(entry) => entry,
            Err(why) => {
                eprintln!("Error reading directory entry: {}", why);
                continue;
            }
        };
        match parse_desktop_file(entry) {
            Some((name, path)) => {
                let entry = map
                    .entry(path.file_name().unwrap().to_string_lossy().to_string())
                    .or_default();
                *entry = (name, path);
            }
            None => (),
        }
    }

    // If a runner is set, use it. If not just print results to stdout
    match env::var("RUNNER_CMD") {
        Ok(cmd) => {
            let mut child = Command::new(cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            child.stdin.take().unwrap().write_all(
                map.iter()
                    .map(|(_, v)| format!("{}\n", v.0))
                    .collect::<String>()
                    .as_bytes(),
            )?;

            match child.wait_with_output() {
                Ok(output) => {
                    let output = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    println!("{}", output);
                    for (_, (name, path)) in map.iter() {
                        if output == *name {
                            // Outsourcing the desktop file running
                            Command::new("dex").arg(path).spawn()?;
                            break;
                        }
                    }
                }
                Err(why) => {
                    println!("Failed to capture child process output: {}", why);
                }
            }
        }
        Err(_) => {
            for (_, v) in map.iter() {
                println!("{}", v.0);
            }
        }
    }

    Ok(())
}

fn parse_desktop_file(entry: &fs::DirEntry) -> Option<(String, PathBuf)> {
    if entry.path().extension() == Some(OsStr::new("desktop")) {
        let mut file = fs::File::open(entry.path()).unwrap();
        let mut buf = String::new();
        file.read_to_string(&mut buf).unwrap();

        let mut exec = None;
        let mut name = None;

        for line in buf.lines() {
            if line.starts_with("Exec=") {
                exec = Some(line.split_once("=").unwrap().1.to_string());
            } else if line.starts_with("Name=") {
                name = Some(line.split_once("=").unwrap().1.to_string());
            }
            if name.is_some() && exec.is_some() {
                break;
            }
        }
        if name.is_some() && exec.is_some() {
            let exec = exec.unwrap();
            return Some((format!("{} ({})", name.unwrap(), exec), entry.path()));
        }
    }
    None
}
