#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]
#![feature(extract_if)]

use anyhow::{ anyhow, bail, Context, Result };
use chrono::{ DateTime, TimeZone, Utc };
use directories::ProjectDirs;
use jsonschema;
use rpassword::prompt_password;
use serde::{ Deserialize, Serialize };
use ssh2::{ FileStat, Session, Sftp };
use std::fs;
use std::io::{ copy, BufReader, Read, Write };
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

const SSH_PORT: &str = ":22";

#[derive(Serialize, Deserialize)]
struct Location {
    name: String,
    local_path: PathBuf,
    remote_path: PathBuf,
    files: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct RemoteSyncHelper {
    auto_sync: bool,
    remote: String,
    user: String,
    locations: Vec<Location>,
}

impl RemoteSyncHelper {
    pub fn init() -> Result<RemoteSyncHelper> {
        let schema_string = include_bytes!("schema/config-schema.json");
        let schema = serde_json::from_slice(schema_string)?;

        let config_default = include_bytes!("config/default.json");

        let proj_dirs = ProjectDirs::from("dev", "sanctified", "remote-sync").unwrap();
        let config_folder = proj_dirs.config_dir();
        let config_path = &config_folder.join("config.json");
        // Linux:   /home/<USER>/.config/remote-sync/config.json
        // Windows: C:\Users\<USER>\AppData\Roaming\sanctified\remote-sync\config.json
        // macOS:   /Users/<USER>/Library/Application Support/dev.sanctified.remote-sync/config.json

        if let Ok(config_file) = fs::File::open(config_path) {
            // Read config file
            let config_reader = BufReader::new(config_file);
            let config = serde_json::from_reader(config_reader)?;
            // Validate config against its schema
            assert!(jsonschema::is_valid(&schema, &config));
            // Parse json into struct
            Ok(serde_json::from_value(config).context("Failed to parse configuration")?)
        } else {
            // Config file doesn't exist, create a new one from template.
            fs::create_dir_all(config_folder)?;
            let mut config_file = fs::File
                ::create_new(config_path)
                .expect("Should have been able to create config file");
            config_file.write_all(config_default)?;

            println!(
                "No configuration file found in '{}'.\nA default configuration was created, edit it and run remote-sync again.",
                config_folder.display()
            );
            bail!("No configuration file")
        }
    }

    pub fn sync_locations(&self) -> Result<()> {
        // Connect to the SSH server
        let tcp = TcpStream::connect(self.remote.to_owned() + SSH_PORT)?;
        let mut session = Session::new()?;
        session.set_tcp_stream(tcp);
        session.handshake()?;
        session.userauth_password(
            self.user.as_str(),
            prompt_password("Enter password:")?.as_str()
        )?;

        for loc in &self.locations {
            match self.sync_location(&session, loc) {
                Ok(()) => {}
                Err(e) => {
                    bail!("Failed to sync {}: {e}", loc.name);
                }
            }
        }
        Ok(())
    }

    fn sync_location(&self, session: &Session, loc: &Location) -> Result<()> {
        let mut files: Vec<(PathBuf, FileStat)> = vec![];
        let handle = session.sftp()?;

        if loc.files.is_empty() {
            files = self.glob_location(&handle, loc)?;
        } else {
            for file in loc.files.as_slice() {
                let path = loc.remote_path.join(file);
                let file_opt = match handle.stat(path.as_path()) {
                    Ok(stat) => Some(stat),
                    Err(e) => {
                        println!("Couldnt find file '{}': {}", path.display(), e);
                        None
                    }
                };
                if file_opt.is_some() {
                    files.push((path, file_opt.unwrap()));
                }
            }
        }

        for remote_file in files {
            let local_file = loc.local_path.join(remote_file.0.strip_prefix(&loc.remote_path)?);
            let local_date = Utc.timestamp_opt(
                fs
                    ::metadata(local_file.as_path())?
                    .accessed()?
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_secs() as i64,
                0
            ).unwrap();
            let remote_date = Utc.timestamp_opt(remote_file.1.atime.unwrap() as i64, 0).unwrap();

            self.sync_file(session, (&local_file, local_date), (&remote_file.0, remote_date))?;
        }

        println!("Synced all files for {}\n", loc.name);
        Ok(())
    }

    fn sync_file(
        &self,
        session: &Session,
        local: (&PathBuf, DateTime<Utc>),
        remote: (&PathBuf, DateTime<Utc>)
    ) -> Result<()> {
        if local.1 == remote.1 {
            // Last access times are the same
            println!("{:?} is up-to-date", local.0.file_name().unwrap().to_str().unwrap());
        } else if local.1 > remote.1 {
            // Remote file is out-of-date
            let mut contents = Vec::new();

            // Open local file and prepare for buffered reading
            let local_file = fs::File
                ::open(local.0)
                .context(format!("Failed to open local file {}", local.0.display()))?;
            let mut buf = BufReader::new(local_file);

            // Get remote file with write access and read contents of local file.
            let mut remote_file = session
                .scp_send(remote.0, 0o644, buf.read_to_end(&mut contents)?.try_into()?, None)
                .context(format!("Failed to open remote file {}", remote.0.display()))?;

            // Write contents of local file into remote file
            remote_file.write_all(&mut contents)?;

            // Properly close channel
            remote_file.send_eof()?;
            remote_file.wait_eof()?;
            remote_file.close()?;
            remote_file.wait_close()?;

            println!("Updated {}", remote.0.display());
        } else {
            // Local file is out-of-date

            // Open local file with write access
            let mut local_file = fs::File
                ::create(local.0)
                .context(format!("Failed to open local file {}", local.0.display()))?;
            let (mut remote_file, _) = session
                .scp_recv(remote.0)
                .context(format!("Failed to open remote file {}", remote.0.display()))?;

            // Copy remote file into local file
            (match copy(&mut remote_file, &mut local_file) {
                Ok(_) => Ok(()),
                Err(e) =>
                    Err(
                        anyhow!(
                            "Failed to copy '{}' to '{}' : {e}",
                            remote.0.display(),
                            local.0.display()
                        )
                    ),
            })?;

            // Properly close channel
            remote_file.send_eof()?;
            remote_file.wait_eof()?;
            remote_file.close()?;
            remote_file.wait_close()?;

            println!("Updated {}", local.0.display());
        }
        Ok(())
    }

    fn glob_location(&self, handle: &Sftp, loc: &Location) -> Result<Vec<(PathBuf, FileStat)>> {
        let mut files = handle.readdir(&loc.remote_path)?;
        let mut dirs: Vec<(PathBuf, FileStat)> = files.extract_if(|f| f.1.is_dir()).collect();

        while dirs.len() != 0 {
            let mut entries = handle.readdir(dirs[0].0.as_path())?;
            dirs.append(&mut entries.extract_if(|f| f.1.is_dir()).collect());
            files.append(&mut entries);
            dirs.remove(0);
        }
        Ok(files)
    }
}

fn main() -> ExitCode {
    if let Ok(helper) = RemoteSyncHelper::init() {
        if helper.auto_sync {
            match helper.sync_locations() {
                Ok(()) => {
                    println!("Great success !");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    println!("While syncing files, {}", e);
                    ExitCode::FAILURE
                }
            }
        } else {
            println!("Nothing to do, autosync is disabled.");
            ExitCode::SUCCESS
        }
    } else {
        ExitCode::FAILURE
    }
}
