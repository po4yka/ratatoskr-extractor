# Legacy shadow comparison report

Legacy archive: `ratatoskr` at `5cdc911b9d33a426613eed79b1ef36db041693e3` (read-only: true).
This is an offline measurement only. Owner approval and a separate cutover change are required before any traffic switch.

## web_article: approve

- samples: 1
- success rate: legacy 1/1 (100.0%), current 1/1 (100.0%)
- minimum content overlap: 0.900
- legacy block statistics:
  - heading: 1
  - paragraph: 4
- current IR block statistics:
  - heading: 1
  - paragraph: 2
- verdict: all committed criteria pass

### Cases
- `web-semantic-article` (https://corpus.example/html-semantic) — legacy success via `app.core.html_utils.html_to_text`, current success, overlap 1.000

## youtube_transcript: approve

- samples: 1
- success rate: legacy 1/1 (100.0%), current 1/1 (100.0%)
- minimum content overlap: 0.950
- legacy block statistics:
  - paragraph: 1
- current IR block statistics:
  - paragraph: 3
- verdict: all committed criteria pass

### Cases
- `youtube-transcript` (https://www.youtube.com/watch?v=dQw4w9WgXcQ) — legacy success via `app.adapters.youtube.youtube_downloader_parts.transcript_api.format_transcript`, current success, overlap 1.000

## x_post: hold

- samples: 1
- success rate: legacy 1/1 (100.0%), current 0/1 (0.0%)
- minimum content overlap: 1.000
- legacy block statistics:
  - paragraph: 3
- current IR block statistics:
  - none
- verdict: current extraction is unsupported: x-post

### Cases
- `x-post` (https://x.com/corpus_author/status/1) — legacy success via `app.adapters.twitter.text_formatter.format_tweets_for_summary`, current unsupported
