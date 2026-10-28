# Test media

One second of a 1 kHz sine peaking at 0.5, made with FFmpeg's `sine`
source and encoders, for the import tests. `surround.flac` is 5.1 with
the sine in the centre channel only; the videos are a plain blue
32×32 picture.

```sh
S="sine=frequency=1000:sample_rate=44100:duration=1"
V="volume=4,pan=stereo|c0=c0|c1=c0"
ffmpeg -f lavfi -i "$S" -af "$V" -c:a libmp3lame -b:a 128k tone.mp3
ffmpeg -f lavfi -i "sine=frequency=1000:sample_rate=48000:duration=1" -af volume=4 -c:a flac -sample_fmt s32 tone.flac
ffmpeg -f lavfi -i "$S" -af "$V" -c:a libvorbis -q:a 3 tone.ogg
ffmpeg -f lavfi -i "$S" -af "$V" -c:a aac -b:a 96k tone.m4a
ffmpeg -f lavfi -i "color=c=blue:s=32x32:r=2:d=1" -f lavfi -i "sine=frequency=1000:sample_rate=48000:duration=1" -af "$V" -c:v mpeg4 -c:a aac -b:a 96k -shortest tone.mp4
ffmpeg -i tone.mp4 -c copy tone.mov
ffmpeg -f lavfi -i "color=c=blue:s=32x32:r=2:d=1" -f lavfi -i "$S" -af "$V" -c:v mpeg4 -c:a libvorbis -shortest tone.mkv
ffmpeg -f lavfi -i "sine=frequency=1000:sample_rate=48000:duration=1" -af "volume=4,pan=5.1|FC=c0" -c:a flac surround.flac
```
