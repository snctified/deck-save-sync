#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use anyhow::{anyhow, Context, Result};
use freya::prelude::launch;
use glob::glob;
use jsonschema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

mod app;

const SCHEMA_FILE_PATH: &str = "src/schema/config-schema.json";
const CONFIG_FILE_PATH: &str = "src/config/test_config.json";

struct DeckSaveButler {
    auto_sync: bool,
    games: Vec<Value>,
}

#[derive(Serialize, Deserialize)]
struct Game {
    id: u64,
    name: String,
    pc_path: String,
    deck_path: String,
    files: Vec<String>,
}

impl DeckSaveButler {
    pub fn init() -> Result<DeckSaveButler> {
        let (auto_sync, games) = Self::get_config().context("Failed to get configuration")?;
        Ok(DeckSaveButler { auto_sync, games })
    }

    fn get_config() -> Result<(bool, Vec<Value>)> {
        let schema_file = fs::File::open(SCHEMA_FILE_PATH)
            .expect("Should have been able to read the schema file");
        let schema_reader = BufReader::new(schema_file);
        let schema = serde_json::from_reader(schema_reader).unwrap();

        let config_file = fs::File::open(CONFIG_FILE_PATH)
            .expect("Should have been able to read the config file");
        let config_reader = BufReader::new(config_file);
        let config = serde_json::from_reader(config_reader).unwrap();

        assert!(jsonschema::is_valid(&schema, &config));

        let auto_sync = config["auto_sync"].as_bool().unwrap();
        let games = config["games"].as_array().unwrap().to_vec();

        Ok((auto_sync, games))
    }

    pub fn sync_games(&self) -> Result<()> {
        for game in &self.games {
            match self.sync_game(game) {
                Ok(()) => {}
                Err(e) => return Err(anyhow!("Failed to sync {}: {e}", game["name"])),
            }
        }
        Ok(())
    }

    fn sync_game(&self, game_json: &Value) -> Result<()> {
        let game: Game = serde_json::from_value(game_json.to_owned())?;

        let pc_path = PathBuf::from(&game.pc_path);
        assert!(pc_path.is_dir());

        let deck_path = PathBuf::from(&game.deck_path);
        assert!(deck_path.is_dir());

        if game.files.is_empty() {
            for entry in glob(pc_path.join("*").to_str().unwrap())? {
                let pc_file = entry?;
                let deck_file = deck_path.join(&pc_file.file_name().unwrap());

                self.sync_files(&pc_file, &deck_file)?;
            }
        } else {
            for file in game.files {
                let pc_file = pc_path.join(&file);
                if !pc_file.is_file() {
                    return Err(anyhow!("File '{}' does not exist", pc_file.display()));
                }
                let deck_file = deck_path.join(&file);
                if !deck_file.is_file() {
                    return Err(anyhow!("File '{}' does not exist", deck_file.display()));
                }

                self.sync_files(&pc_file, &deck_file).with_context(|| {
                    format!(
                        "Failed to sync files {} and {}",
                        pc_file.to_str().unwrap(),
                        deck_file.to_str().unwrap()
                    )
                })?;
            }
        }
        Ok(())
    }

    fn sync_files(&self, a: &PathBuf, b: &PathBuf) -> Result<()> {
        let a_meta = fs::metadata(&a).context(format!("File '{}' not found", a.display()))?;
        let b_meta = fs::metadata(&b).context(format!("File '{}' not found", b.display()))?;

        if a_meta.accessed()? > b_meta.accessed()? {
            fs::copy(&a, &b).context(format!(
                "Failed to copy '{}' to '{}'",
                a.display(),
                b.display()
            ))?;
        } else {
            fs::copy(&b, &a).context(format!(
                "Failed to copy '{}' to '{}'",
                b.display(),
                a.display()
            ))?;
        }
        Ok(())
    }
}

fn main() {
    let butler = DeckSaveButler::init().unwrap();

    if butler.auto_sync {
        match butler.sync_games() {
            Ok(()) => (),
            Err(e) => println!("While syncing games, {}", e),
        }
    } else {
        launch(app::app); // Be aware that this will block the thread
    }
}
