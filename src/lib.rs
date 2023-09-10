#![allow(unused)]

use std::fs;
use std::io;
use std::io::Cursor;
use std::panic;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::anyhow;
use anyhow::bail;
use attohttpc::Session;
use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::TimeZone;
use chrono::Utc;
use encoding_rs::SHIFT_JIS;
use pickledb::PickleDb;
use pickledb::PickleDbDumpPolicy;
use pickledb::SerializationMethod;
use serde::de;
use serde::Deserialize;
use serde::Deserializer;
use tracing::info;
use tracing::warn;
use zip::ZipArchive;

const NAUTICA_BASE_URL: &str = "https://ksm.dev";

#[derive(Debug, Deserialize)]
struct Song {
    id: String,
    user_id: String,
    title: String,
    artist: String,
    #[serde(deserialize_with = "datetime_from_uploaded_at")]
    uploaded_at: DateTime<Utc>,
}

fn datetime_from_uploaded_at<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let uploaded_at: String = de::Deserialize::deserialize(deserializer)?;
    let datetime = NaiveDateTime::parse_from_str(&uploaded_at, "%Y-%m-%d %H:%M:%S")
        .map_err(de::Error::custom)?;
    Ok(Utc.from_utc_datetime(&datetime))
}

#[derive(Debug, Deserialize)]
struct Links {
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SongsResp {
    data: Vec<Song>,
    links: Links,
}

pub struct Downloader {
    /// Destination directory to save songs.
    dest: PathBuf,

    /// Base URL of the Nautica app server.
    base_url: String,

    sess: Session,
}

impl Downloader {
    pub fn builder() -> DownloaderBuilder {
        DownloaderBuilder::default()
    }

    pub fn download_all(&self) -> anyhow::Result<()> {
        let mut db = PickleDb::load_json(self.dest.join("meta.json"), PickleDbDumpPolicy::AutoDump)
            .unwrap_or_else(|_| {
                PickleDb::new_json(self.dest.join("meta.json"), PickleDbDumpPolicy::AutoDump)
            });
        let mut next_link = format!("{}/app/songs?sort=uploaded", self.base_url);

        'outer: loop {
            let resp = self.sess.get(&next_link).send()?;
            let songs_resp: SongsResp = resp.json_utf8()?;

            for song in songs_resp.data {
                let song_dest = self.dest.join(&song.id);

                if db.get::<DateTime<Utc>>(&song.id).is_some() {
                    info!(
                        title = song.title,
                        artist = song.artist,
                        "This song already exists. Cancel the remaining downloads."
                    );
                    break 'outer;
                }

                info!(title = song.title, artist = song.artist, "Downloading");

                if self.download(&song.id).is_ok() {
                    db.set(&song.id, &Utc::now())?;
                } else {
                    warn!("Failed to download");
                }
            }

            if let Some(next) = songs_resp.links.next {
                next_link = next;
            } else {
                break;
            };
        }
        Ok(())
    }

    fn download(&self, song_id: &str) -> anyhow::Result<()> {
        let resp = self
            .sess
            .get(format!("{}/songs/{}/download", self.base_url, song_id))
            .send()?;
        let dest = self.dest.join(song_id);
        if !dest.exists() {
            fs::create_dir(&dest)?;
        }

        let mut archive = ZipArchive::new(Cursor::new(resp.bytes()?))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;

            if file.name().ends_with('/') {
                continue;
            };

            let filepath = {
                let (cow, _, had_errors) = SHIFT_JIS.decode(file.name_raw());
                let enclosed_name = if had_errors {
                    file.enclosed_name()
                } else {
                    enclosed_name(&cow)
                };
                match enclosed_name {
                    Some(path) => path.to_owned(),
                    None => {
                        warn!(path = file.name(), "invalid file path");
                        continue;
                    }
                }
            };

            let filename = filepath.file_name().unwrap().to_str().unwrap();
            let mut outfile = fs::File::create(dest.join(filename))?;
            io::copy(&mut file, &mut outfile)?;
        }

        Ok(())
    }
}

fn enclosed_name(file_name: &str) -> Option<&Path> {
    if file_name.contains('\0') {
        return None;
    }
    let path = Path::new(file_name);
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return None,
            Component::ParentDir => depth = depth.checked_sub(1)?,
            Component::Normal(_) => depth += 1,
            Component::CurDir => (),
        }
    }
    Some(path)
}

#[derive(Debug)]
pub struct DownloaderBuilder {
    dest: PathBuf,
    base_url: String,
}

impl DownloaderBuilder {
    pub fn dest<P: Into<PathBuf>>(mut self, dest: P) -> Self {
        self.dest = dest.into();
        self
    }

    pub fn base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    pub fn build(self) -> Downloader {
        Downloader {
            dest: self.dest,
            base_url: self.base_url,
            sess: Session::new(),
        }
    }
}

impl Default for DownloaderBuilder {
    fn default() -> Self {
        Self {
            dest: PathBuf::from("nautica"),
            base_url: String::from(NAUTICA_BASE_URL),
        }
    }
}

#[cfg(test)]
mod test {
    use std::fs::File;

    use httpmock::MockServer;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parse_songs_resp() {
        let songs: SongsResp =
            serde_json::from_reader(File::open("tests/fixtures/songs.json").unwrap()).unwrap();
        assert_eq!(songs.data.len(), 10);
        assert_eq!(
            songs.data[0].uploaded_at,
            Utc.with_ymd_and_hms(2023, 9, 7, 5, 56, 46).unwrap()
        );
    }

    // #[test]
    // fn download() {
    //     let server = MockServer::start();
    //     let m = server.mock(|when, then| {
    //         when.path("/songs/5441d590-4d43-11ee-a602-d95b1bfc2e6d/download");
    //         then.header("content-type", "application/x-zip")
    //             .status(200)
    //             .body(include_bytes!(
    //                 "../tests/fixtures/5441d590-4d43-11ee-a602-d95b1bfc2e6d.zip"
    //             ));
    //     });

    //     let dest = tempdir().unwrap();

    //     let downloader = Downloader::builder()
    //         .dest(dest.path())
    //         .base_url(server.base_url())
    //         .build();

    //     let song = Song {
    //         id: "5441d590-4d43-11ee-a602-d95b1bfc2e6d".to_string(),
    //         user_id: "afc379f0-79ee-11eb-a306-21913834edef".to_string(),
    //         title: "Outbreak".to_string(),
    //         artist: "RG+Ice".to_string(),
    //         uploaded_at: Utc.with_ymd_and_hms(2023, 9, 7, 5, 56, 46).unwrap(),
    //     };
    //     downloader.download(&song);

    //     m.assert();
    // }

    #[test]
    fn download() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.path("/songs/89b54d80-4e6d-11ee-83d4-2ffdf82667a6/download");
            then.header("content-type", "application/x-zip")
                .status(200)
                .body(include_bytes!(
                    "../tests/fixtures/89b54d80-4e6d-11ee-83d4-2ffdf82667a6.zip"
                ));
        });

        let dest = tempdir().unwrap();

        let downloader = Downloader::builder()
            .dest(dest.path())
            .base_url(server.base_url())
            .build();

        // let song = Song {
        //     id: "89b54d80-4e6d-11ee-83d4-2ffdf82667a6".to_string(),
        //     user_id: "87cd71e0-d13e-11eb-ac5e-f190cbe4b837".to_string(),
        //     title: "チューリングラブ feat.Sou".to_string(),
        //     artist: "ナナヲアカリ".to_string(),
        //     uploaded_at: Utc.with_ymd_and_hms(2023, 9, 8, 17, 31, 26).unwrap(),
        // };
        downloader
            .download("89b54d80-4e6d-11ee-83d4-2ffdf82667a6")
            .unwrap();

        m.assert();

        let song_dest = dest.path().join("89b54d80-4e6d-11ee-83d4-2ffdf82667a6");
        assert_eq!(song_dest.read_dir().unwrap().collect::<Vec<_>>().len(), 9);
        assert!(song_dest.join("3.wav").exists());
        assert!(song_dest.join("5.wav").exists());
        assert!(song_dest.join("6.wav").exists());
        assert!(song_dest.join("8.wav").exists());
        assert!(song_dest.join("10.wav").exists());
        assert!(song_dest.join("chart.png").exists());
        assert!(song_dest.join("チューリングラブ feat.Sou.ksh").exists());
        assert!(song_dest.join("チューリングラブ feat.Sou.ogg").exists());
        assert!(song_dest.join("チューリングラブ feat.Sou.png").exists());
    }
}
