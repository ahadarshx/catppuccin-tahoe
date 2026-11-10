//! Tune's own styles, layered over the theme.

pub const CSS: &str = "
.album-art { border-radius: 8px; }
.album-art.large { border-radius: 12px; }
.album-art.huge { border-radius: 14px; box-shadow: 0 18px 48px alpha(black, 0.45); }
.album-art.round { border-radius: 9999px; }
.album-art.placeholder { background-color: alpha(currentColor, 0.08); }

.song-row.drop-above { box-shadow: inset 0 2px 0 var(--accent-color); }
.song-row.drop-below { box-shadow: inset 0 -2px 0 var(--accent-color); }

.mini-player { padding: 16px; }
.mini-player .mini-title { font-weight: bold; }
.mini-player .mini-seek { padding: 0; margin-top: 4px; }
.mini-player .mini-seek slider {
  min-width: 12px; min-height: 12px; margin: -4px;
  background-color: var(--window-fg-color); border: none; box-shadow: none;
}
.mini-header { background: none; box-shadow: none; min-height: 0; }

/* The player floats over the pages: the bar area itself is see-through. */
toolbarview.player-host .bottom-bar { background: none; box-shadow: none; border: none; }
.player-bar {
  margin: 0 10px 10px 10px;
  /* Even inset: the cover's 8px corners sit concentric in the card's 20px ones. */
  padding: 12px 20px 12px 12px;
  border-radius: 20px;
  background-color: var(--popover-bg-color);
  box-shadow: 0 8px 28px alpha(black, 0.45), inset 0 0 0 1px alpha(white, 0.06);
  transition: background-color 400ms ease-out;
}
.player-bar.stacked { padding: 10px 14px 12px 10px; }
.player-bar .title { font-weight: bold; }
.player-bar togglebutton:checked, .player-bar button.on {
  color: var(--accent-color);
  background-color: alpha(var(--accent-color), 0.15);
}
.player-bar button { transition: background-color 150ms ease-out, color 150ms ease-out; }
.player-bar .cover-button { padding: 0; border-radius: 8px; background: none; box-shadow: none; }
.player-bar .cover-button:hover .album-art { filter: brightness(1.12); }
.player-bar .like-button:checked { background: none; }
.player-bar .like-button.liked { color: var(--accent-color); }
.connect-badge { color: var(--accent-color); }

/* macOS-style sliders: a thin rounded bar with no knob, a little thicker
   while the pointer is on it. */
scale.flat-slider { padding: 6px 0; }
scale.flat-slider > trough {
  min-height: 4px;
  border-radius: 9999px;
  background-color: alpha(currentColor, 0.16);
  transition: min-height 120ms ease-out;
}
scale.flat-slider > trough > highlight {
  min-height: 4px;
  border-radius: 9999px;
  background-color: alpha(currentColor, 0.62);
  transition: min-height 120ms ease-out, background-color 120ms ease-out;
}
scale.flat-slider > trough > slider {
  min-width: 0; min-height: 0; margin: 0; padding: 0;
  border: none; background: none; box-shadow: none; outline: none;
  opacity: 0;
}
scale.flat-slider:hover > trough, scale.flat-slider:active > trough,
scale.flat-slider:hover > trough > highlight, scale.flat-slider:active > trough > highlight {
  min-height: 7px;
}
scale.flat-slider:hover > trough > highlight, scale.flat-slider:active > trough > highlight {
  background-color: currentColor;
}
scale.flat-slider:disabled > trough > highlight { background-color: transparent; }
scale.flat-slider:focus-visible > trough { outline: 2px solid alpha(var(--accent-color), 0.6); outline-offset: 2px; }

/* Links inside labels: the artist and album names. */
label.links link { color: inherit; text-decoration: none; }
label.links link:hover { text-decoration: underline; }

/* Covers only: a narrow column with everything centred and evenly spaced. */
.sidebar-pane list.navigation-sidebar.compact { padding: 6px 0; }
.sidebar-pane list.navigation-sidebar.compact > row { padding: 6px; margin: 3px 0; border-radius: 12px; }
.sidebar-pane list.navigation-sidebar > row { transition: background-color 150ms ease-out; }
/* No hairline under the headers when a page scrolls: just the soft fade. */
scrolledwindow > undershoot.top, scrolledwindow > undershoot.bottom { box-shadow: none; }
/* Full sidebar: even breathing room around each cover inside its highlight. */
.sidebar-pane list.navigation-sidebar:not(.compact) { padding: 8px 12px; }
.sidebar-pane list.navigation-sidebar:not(.compact) > row { padding: 6px; margin: 2px 0; border-radius: 12px; }
.sidebar-tile { border-radius: 8px; }
.sidebar-tile.icon-tile { background-color: alpha(currentColor, 0.08); }
.liked-tile {
  background-image: linear-gradient(135deg, #4b1fd1, #8d8ce6 65%, #c5e6d8);
  color: white;
}

/* Song lists: the rows draw their own highlight, so headers and shelves
   in the same list stay plain. */
listview.songs { background: none; }
listview.songs > row { padding: 0; background: none; border-radius: 10px; }
listview.songs > row:focus-visible { outline: 2px solid alpha(var(--accent-color), 0.6); outline-offset: -2px; }
list.songs { background: none; }
list.songs > row { padding: 0; background: none; border-radius: 10px; }
.song-row { padding: 6px 10px; border-radius: 10px; transition: background-color 120ms ease-out; }
listview.songs > row:hover .song-row, list.songs > row:hover .song-row { background-color: alpha(currentColor, 0.07); }
.song-row .title { font-weight: 500; }
.song-row.now-playing .title { color: var(--accent-color); font-weight: bold; }
.song-row .track-number { min-width: 44px; font-feature-settings: 'tnum'; }
.song-row.now-playing .track-number { color: var(--accent-color); }
.song-row .more { opacity: 0; transition: opacity 120ms ease-out; }
listview.songs > row:hover .song-row .more, list.songs > row:hover .song-row .more,
.song-row .more:checked, .song-row .more:focus-visible { opacity: 1; }
.track-duration { font-feature-settings: 'tnum'; }
/* A play icon over the cover on hover, a speaker while playing. */
.row-play {
  border-radius: 8px;
  background-color: alpha(black, 0.45);
  color: white;
  opacity: 0;
  transition: opacity 150ms ease-out;
}
listview.songs > row:hover .row-play, list.songs > row:hover .row-play, .song-row.now-playing .row-play { opacity: 1; }

/* Page headers over albums, playlists and artists. */
.page-header { padding: 8px 4px 18px 4px; }
.page-header .kind { font-weight: bold; font-size: 9pt; letter-spacing: 1px; }
.page-header .header-title { font-size: 34pt; font-weight: 800; }
.page-header .header-title.long { font-size: 24pt; }
.page-header .big-play { min-width: 52px; min-height: 52px; padding: 0; }
.page-header .big-play image { -gtk-icon-size: 22px; }
.section-title { font-size: 15pt; font-weight: bold; margin-top: 18px; }

/* Home shelf items: no box of their own, just cover and text. */
flowbox.shelf > flowboxchild { background: none; padding: 0; }
.shelf-item, .shelf-item:hover, .shelf-item:active { padding: 4px; background: none; box-shadow: none; }
.shelf-item .album-art { transition: filter 200ms ease-out; }
.shelf-item:hover .album-art { filter: brightness(1.08); }
/* Spotify's play button: rises into view on hover. */
.shelf-item .card-play {
  min-width: 42px;
  min-height: 42px;
  margin: 0 10px 0 0;
  border-radius: 9999px;
  background-color: var(--accent-bg-color);
  color: var(--accent-fg-color);
  box-shadow: 0 6px 14px alpha(black, 0.35);
  opacity: 0;
  transition: opacity 180ms ease-out, margin 180ms ease-out;
}
.shelf-item:hover .card-play { opacity: 1; margin-bottom: 10px; }
.shelf-item .card-title { font-weight: bold; }
.greeting { font-size: 26pt; font-weight: 800; }

/* Placeholders while a page loads. */
@keyframes skeleton-pulse { from { opacity: 0.45; } to { opacity: 1; } }
.skeleton {
  background-color: alpha(currentColor, 0.08);
  border-radius: 8px;
  animation: skeleton-pulse 850ms ease-in-out infinite alternate;
}
.skeleton.line { border-radius: 9999px; }

/* Now Playing: the cover, large, over a blurred copy of itself. */
.now-playing-backdrop { filter: blur(60px) saturate(1.4); opacity: 0.55; }
.now-playing-shade { background-image: linear-gradient(to bottom, alpha(black, 0.05), alpha(black, 0.45)); }
.now-playing-title { font-size: 26pt; font-weight: 800; }
.now-playing-artist { font-size: 14pt; }

/* Lyrics: big bold lines; the one being sung is bright, sung ones fade. */
.timing-pill {
  padding: 3px;
  border-radius: 9999px;
  background-color: var(--popover-bg-color);
  box-shadow: inset 0 0 0 1px alpha(white, 0.06);
}
.timing-pill button { min-width: 28px; min-height: 28px; padding: 0; }
.timing-pill button.timing-value { padding: 0 6px; border-radius: 9999px; background: none; }
.timing-pill button.timing-value:hover { background-color: alpha(currentColor, 0.08); }
.timing-pill .shifted { color: var(--accent-color); }
.lyrics-view .lyric-text { font-size: 22pt; font-weight: 800; }
.lyrics-view .lyric-line { padding: 6px 12px; border-radius: 14px; color: alpha(currentColor, 0.42); transition: color 300ms ease-out, background-color 150ms ease-out; }
.lyrics-view .lyric-line.past { color: alpha(currentColor, 0.26); }
.lyrics-view .lyric-line.current { color: currentColor; }
.lyrics-view .lyric-line:hover { color: alpha(currentColor, 0.85); background-color: alpha(currentColor, 0.06); }
.lyrics-view .plain { font-size: 17pt; font-weight: 700; padding: 3px 12px; color: alpha(currentColor, 0.85); }

/* Queue */
.queue-heading { font-weight: bold; margin: 18px 6px 4px 6px; }
";
