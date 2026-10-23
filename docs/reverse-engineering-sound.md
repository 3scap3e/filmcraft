# Reverse Engineering: Sound

The Sound tool reads the mix of the selected timeline range the way the Look tool reads its
pictures: one analysis of the whole range, then one description per shot, every word backed by a
number and a stated rule, exported as a `filmcraft.sound` file that a person, a language model or
a generator can use. Pure Rust (`crates/sound`, layer 1, dependency-free apart from the audio DSP
crate). The only model involved is the optional Whisper transcriber the Text panel already uses,
for dialogue.

## What Analyse does

1. **Detects the shots first** when there is no current Shots & look result for the range (the
   same chaining as the Look tool), so the sound can be read per shot.
2. **Mixes the range** through the sequence mixer (all audio tracks, effects and volume as they
   play) at the sequence's sample rate, in 48 000-frame steps, under the 1 hour cap.
3. **Measures** (`filmcraft_sound::analyze`):
   - *Loudness* per ITU-R BS.1770-4 / EBU R128: integrated, loudness range, true peak, the maximum
     momentary and short-term values and the momentary curve every 100 ms.
   - *Features per hop* (1 024 samples, a 2 048-sample Hann frame): RMS level, spectral centroid,
     spectral flatness, flux, zero-crossing rate, power shares below 250 Hz / 250–4 000 Hz / above
     4 000 Hz and an onset strength.
   - *Segments* of about one second labelled **silence, speech, music, effects or ambience** by
     scoring rules on level modulation, pause share, flatness, band shares, peak-over-mean and
     onset rate; the label is the best score, the confidence its margin over the runner-up.
     Consecutive segments of one kind merge into *regions*.
   - *Tempo and beats*: autocorrelation of the onset strength over the BPM range (default
     60–200) with a preference around 120 BPM, then a dynamic-programming beat tracker (Ellis
     2007) that follows the onsets instead of laying a rigid grid.
   - *Cuts on beats*: how many shot boundaries fall within the beat tolerance (default 80 ms) of a
     beat, with the signed offset of every cut.
4. **Transcribes the dialogue** when *Transcribe dialogue* is on and a speech model is installed
   (Settings ▸ Media Analysis & Transcription): the mix is downmixed to 16 kHz mono, the words are
   grouped into lines at speaker changes and pauses over 0.6 s, and every line is attached to the
   shot it starts in. Without a model the result says so in `dialogueSkipped`.
5. **Per shot**: the time share of every kind, the dominant kind, the energy-mean and maximum
   momentary loudness, the beats inside the shot and its dialogue lines.

The page shows the verdict (the one-line summary), a strip with the regions in colour, the
loudness curve, the beats and the shot boundaries, then the shots with the selected shot's own
strip and lines. *Details* lists every number, the regions, the dialogue, the rules and the
limitations. *Advanced* holds the segment length, the tempo range, the beat tolerance and the
dialogue switch; changing any of them makes the result stale.

## Actions

| Button | Command | What it does |
|---|---|---|
| Analyse / Analyse again | `reverseEngineering.runSound` | Runs the steps above (chains `runShots` when needed) |
| Cancel | `reverseEngineering.cancelSound` | Stops the mix, the analysis or the transcription; the previous result is kept |
| Export sound… | `reverseEngineering.exportSound {"path"?}` | Writes the `filmcraft.sound` file |
| Copy description | `reverseEngineering.copySoundDescription` | Copies the same JSON to the clipboard |
| Beat markers | `reverseEngineering.soundBeatMarkers` | Adds a sequence marker on every beat of the grid inside the range (`markers.add`, up to 4 096) |
| Save result / Delete result | `reverseEngineering.saveToolResult` / `deleteToolResult` | Keeps or forgets the result with the project, like every other tool |
| Shot row | `reverseEngineering.soundShot {"index"}` | Selects the shot and moves the playhead to its start |
| Advanced | `reverseEngineering.setSoundOptions {"segmentSeconds"?,"minBpm"?,"maxBpm"?,"beatTolerance"?,"dialogue"?}` | Validated: segment 0.25–10 s, tempo 30–300 BPM with min < max, tolerance 0–1 s |

Automation ids: `reverseEngineering.runSound`, `.cancelSound`, `.sound.export`,
`.soundBeatMarkers`, `.copySoundDescription`, `.sound.strip`, `.sound.shot.N`,
`.sound.shot.N.strip`, `.sound.shot.N.line.M`, `.sound.keyframe`, `.sound.segmentSeconds`,
`.sound.minBpm`, `.sound.maxBpm`, `.sound.beatTolerance`, `.sound.dialogue`, plus the shared
`.status`, `.verdict`, `.quality`, `.details`, `.advanced`, `.tool.back`.

## The `filmcraft.sound` file

```json
{
  "format": "filmcraft.sound",
  "version": 1,
  "source": { "sequence": "Main Edit", "sampleRate": 48000, "channels": 2, "frameRate": 23.976,
              "startFrame": 0, "endFrame": 719, "startSeconds": 0.0, "seconds": 30.03,
              "analyzer": "filmcraft-sound/mix-v1" },
  "summary": "48.0 kHz stereo · 30.0 s · -18.2 LUFS integrated, range 6.1 LU, true peak -1.3 dBTP · speech 52 %, music 38 %, silence 10 % · 118 BPM (confidence 0.61), 59 beats · 4 of 6 cuts on a beat · 9 dialogue lines",
  "loudness": { "integratedLufs": -18.2, "rangeLu": 6.1, "truePeakDbtp": -1.3, "maxMomentaryLufs": -12.0,
                "maxShortTermLufs": -14.1, "curveSeconds": 0.1, "momentaryCurve": [ -120.0, -31.2, "…" ] },
  "shares": { "silence": 0.10, "speech": 0.52, "music": 0.38, "effects": 0.0, "ambience": 0.0 },
  "tempo": { "bpm": 118.0, "periodSeconds": 0.508, "confidence": 0.61, "beats": [ 0.31, 0.82, "…" ] },
  "cutsOnBeats": { "total": 6, "onBeat": 4, "toleranceSeconds": 0.08, "share": 0.667, "offsetsMs": [ 12.0, -210.5, "…" ] },
  "regions": [ { "index": 0, "kind": "music", "startSeconds": 0.0, "endSeconds": 4.1, "segments": 4, "meanConfidence": 0.42, "meanDb": -22.5 } ],
  "segments": [ { "index": 0, "startSeconds": 0.0, "endSeconds": 1.02, "kind": "music", "confidence": 0.4,
                  "meanDb": -22.1, "peakDb": -18.0, "modulationDb": 2.3, "pauseShare": 0.0, "centroidHz": 1850.0,
                  "flatness": 0.06, "flux": 0.12, "zcr": 0.08, "lowShare": 0.3, "speechShare": 0.55, "highShare": 0.15,
                  "onsetRate": 2.0, "scores": { "speech": 0.2, "music": 0.6, "effects": 0.1, "ambience": 0.3 } } ],
  "shots": [ { "index": 0, "startFrame": 0, "endFrame": 119, "startTick": 0, "endTick": 1270080000000, "keyframe": 60,
               "sound": { "startSeconds": 0.0, "endSeconds": 5.0, "shares": { "…": 0 }, "dominant": "music",
                          "meanLufs": -20.4, "maxMomentaryLufs": -15.2, "beats": 10, "summary": "music 100 % · -20.4 LUFS · 10 beats" },
               "lines": [ { "index": 0, "speaker": 0, "startSeconds": 1.2, "endSeconds": 2.9, "text": "Hello there.", "words": 2, "confidence": 0.91 } ] } ],
  "dialogue": [ "… every line of the range …" ],
  "rules": [ { "name": "speech", "rule": "0.45 × ramp(modulation 4→10 dB) + …" } ],
  "limitations": [ "…" ]
}
```

Times inside the file are seconds from the start of the analysed range; `startSeconds` in
`source` places that start on the timeline, and the shots carry exact ticks too.

## In the AI brief

**Export AI brief** on the tool menu (`reverseEngineering.exportBrief`) takes the sound result
along when one is current or saved for the same captures as the Shots & look result. The brief's
shots come from Shots & look, so every one of them gets its sound read from the analysis over its
own time span (`filmcraft_sound::range_sound`): the kind shares, the dominant kind, the mean
momentary loudness, the beats inside the shot and the dialogue lines that start in it. In the
Markdown that is a *Sound* column in the shot table, a `- Sound:` line and `- Line:` entries under
every shot (black shots included), `sound <kind>` in the prompt scaffold, and a **Sound** section
before the limitations with the whole range's loudness, mix, tempo, cuts on beats and the number
of dialogue lines (or why none were transcribed). The JSON carries the same block as
`input.sound` (`BriefInput::sound` in `filmcraft_vision::brief`: `analyzer`, `seconds`,
`integratedLufs`, `rangeLu`, `truePeakDbtp`, `shares`, `tempoBpm`, `tempoConfidence`, `beats`,
`cutsOnBeats`, `dialogueLines`, `dialogueSkipped`, `shots[]` with `shotIndex`, `shares`,
`dominant`, `meanLufs`, `beats`, `lines`). The sound's limitations join the brief's. Without a
sound result the brief is unchanged and the export reports `"sound": false`.

## Honesty

- The classifier is a set of named heuristics, not a trained model. Sung vocals can read as
  speech, dense percussion as effects, a quiet pad as ambience. The confidence is the margin
  between the two best scores, so a 0.05 means "could be either".
- The tempo prefers tempi near 120 BPM; half and double tempo are the classic failure. The beat
  tracker follows the onsets, so the grid is only as good as the onsets.
- The analysis runs on the mix, not on stems. A voice over music is labelled by whichever
  dominates each second; the dialogue lines come from the transcriber regardless of the label.
- Loudness is the sequence mix as rendered; master effects apply, the output device does not.
