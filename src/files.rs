//! Atomic JSON snapshots with a private staging file in the destination directory.
use serde::{de::DeserializeOwned, Serialize};
use std::{
  fs::File,
  io::{BufReader, Write},
  path::Path,
};

struct PendingWrite<'a> {
  destination: &'a Path,
  staging: tempfile::NamedTempFile,
}

impl<'a> PendingWrite<'a> {
  fn begin(destination: &'a Path) -> Result<Self, &'static str> {
    let directory = destination.parent().ok_or("Invalid data directory")?;
    std::fs::create_dir_all(directory).map_err(|_| "Cannot create plugin data directory")?;
    let staging =
      tempfile::NamedTempFile::new_in(directory).map_err(|_| "Cannot create private state file")?;
    Ok(Self { destination, staging })
  }

  fn encode(&mut self, record: &impl Serialize) -> Result<(), &'static str> {
    serde_json::to_writer(&mut self.staging, record).map_err(|_| "Cannot encode plugin state")
  }

  fn commit(mut self) -> Result<(), &'static str> {
    self.staging.flush().map_err(|_| "Cannot write plugin state")?;
    self.staging.as_file().sync_all().map_err(|_| "Cannot sync plugin state")?;
    self.staging.persist(self.destination).map_err(|_| "Cannot save plugin state")?;
    Ok(())
  }
}

pub(crate) fn store_json(destination: &Path, record: &impl Serialize) -> Result<(), String> {
  let mut transaction = PendingWrite::begin(destination)?;
  transaction.encode(record)?;
  transaction.commit().map_err(str::to_owned)
}

pub(crate) fn load_json<T: DeserializeOwned>(source: &Path) -> Result<Option<T>, String> {
  let input = match File::open(source) {
    Ok(file) => file,
    Err(error) => {
      return if error.kind() == std::io::ErrorKind::NotFound {
        Ok(None)
      } else {
        Err("Cannot read plugin state".into())
      }
    }
  };
  serde_json::from_reader(BufReader::new(input)).map(Some).map_err(|error| {
    if error.is_io() { "Cannot read plugin state" } else { "Plugin state is invalid" }.to_owned()
  })
}
