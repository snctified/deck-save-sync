#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use freya::prelude::launch;
use jsonschema;
use serde_json::Value;
use std::fs::File;
use std::io::BufReader;

mod app;

const SCHEMA_FILE_PATH: &str = "src/schema/config-schema.json";
const CONFIG_FILE_PATH: &str = "src/config/config.json";

struct DeckSaveButler {
    auto_sync: bool,
    games: Vec<Value>,
}

impl DeckSaveButler {
    pub fn init() -> DeckSaveButler {
        let (auto_sync, games) = Self::get_config();
        return DeckSaveButler { auto_sync, games };
    }

    fn get_config() -> (bool, Vec<Value>) {
        let schema_file =
            File::open(SCHEMA_FILE_PATH).expect("Should have been able to read the schema file");
        let schema_reader = BufReader::new(schema_file);
        let schema = serde_json::from_reader(schema_reader).unwrap();

        let config_file =
            File::open(CONFIG_FILE_PATH).expect("Should have been able to read the config file");
        let config_reader = BufReader::new(config_file);
        let config = serde_json::from_reader(config_reader).unwrap();

        assert!(jsonschema::is_valid(&schema, &config));

        let auto_sync = config["autoSync"].as_bool().unwrap();
        let games = config["games"].as_array().unwrap().to_vec();

        return (auto_sync, games);
    }

    pub fn sync_games(&self) -> bool {
        let mut result = true;

        for game in &self.games {
            println!("{}", game["_id"]);
            result &= self.sync_game(game["_id"].as_u64().unwrap())
        }

        return result;
    }

    fn sync_game(&self, _id: u64) -> bool {
        return true;
    }

}

fn main() {
    let butler = DeckSaveButler::init();
    
    if butler.auto_sync {
        butler.sync_games();
    } else {
        launch(app::app); // Be aware that this will block the thread
    }
}
