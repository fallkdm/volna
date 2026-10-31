//! Разбор ответов YouTube Music: JSON InnerTube и HTML-фолбэки.
//!
//! Модуль разбит по доменам: треки, альбомы, исполнители, плейлисты,
//! поиск, радио, HTML-фолбэки и общие утилиты. Публичный API
//! (`parse_*`, `extract_*`, `find_*`) реэкспортируется наверх, поэтому
//! `use crate::api::ytmusic::parser::*` продолжает работать без изменений.

use crate::api::models::{
    Album, Artist, ArtistBrief, ArtistCover, Playlist, SearchResult, SearchSection, Track,
    TrackSource,
};

mod album;
mod artist;
mod common;
mod html;
mod library;
mod playlist;
mod radio;
mod search;
mod stream;
mod thumbnails;
mod track;

#[cfg(test)]
mod tests;

pub use album::*;
pub use artist::*;
pub use common::*;
pub use html::*;
pub use library::*;
pub use playlist::*;
pub use radio::*;
pub use search::*;
pub use stream::*;
pub use thumbnails::*;
pub use track::*;
