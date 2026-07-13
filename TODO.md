# Feature: DVD deinterlacing

Interlaced/telecined MPEG-2 DVDs show combing when re-encoded (the pipeline has no
deinterlacer). Add an optional bwdif filter stage to the transcode pipeline for DVD
sources. Workaround until then: `video_codec = "copy"`.

# Feature: Distributed transcoding

As a user i want to be able to Read a BD title on one computer and then transcode it on one (or more) computers.

## Reading the title

One computer with the bd drive reads the bd, its metadata, creates a ffmpeg "job" and reads the raw title into a shared network dir. Then its saves the "job" data also into the dir.

The job should everything from target tracks, presets, crf etc

## Transcoding

The other computer with e.g a better CPU/GPU reads the "job" configuration and does the transcoding. Multiple computer could work on multiple files at once.

- Each computer defines its own bdrip config, as the directories may change
- Each job must handle a "lock" (to prevent one file to be ripped multiple times) and handle errors / aborts to prevent "dead" jobs

