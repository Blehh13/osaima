# Wake-Word Data & Training Guide — "Hey OSAIMA"

The one dataset the assistant **cannot** generate: it needs real microphone
audio. This guide + `record_wakeword.py` is everything you need to produce it
yourself tonight (~20 min of recording).

## 1. Why audio can't be synthesized here
openWakeWord trains on short clips of *your* voice saying the wake phrase, plus
negatives (other speech, silence, noise). Text can't stand in for a waveform, so
you record it. Target from the PDR: **≥ 50 positive samples** (more = better).

## 2. Capture (run at your mic)
```bash
pip install sounddevice soundfile numpy
cd ai-core/asr

# positives — say "Hey OSAIMA" each time (vary tone, distance, speed)
python record_wakeword.py --label positive --count 60

# negatives — other phrases, silence, background noise, TV, typing
python record_wakeword.py --label negative --count 150
```
Produces `positives/*.wav` and `negatives/*.wav` at 16 kHz mono PCM16.

### Recording tips (improves real-world accuracy)
- Vary distance from mic (0.3 m – 3 m) and speaking volume.
- Record a few with background noise (fan, music) — matches real use.
- Include 2–3 other speakers if available (household members).
- Negatives should include near-misses: "hey Google", "hey", "OSAIMA" alone.

## 3. Augment (optional, recommended)
openWakeWord's pipeline can multiply your 60 clips into thousands by mixing in
room impulse responses and noise. Use their synthetic-data + augmentation
notebook, or:
```bash
pip install openwakeword
python -m openwakeword.train.augment --input positives/ --output augmented/
```

## 4. Train the custom model
Follow the official openWakeWord training flow:
```bash
# clone the training utilities
pip install openwakeword[training]
# point the config at positives/, negatives/, augmented/
python -m openwakeword.train \
    --model_name hey_osaima \
    --positive_dir positives/ \
    --negative_dir negatives/ \
    --output hey_osaima.onnx
```
Output `hey_osaima.onnx` drops into the ASR daemon as the wake model.

## 5. Validate against the PDR target
- **False wake-word rate < 1 per hour idle** (PDR §9.1).
- Test: record 8 h of ambient room audio, run the model offline, count triggers.
```bash
python -m openwakeword.detect --model hey_osaima.onnx --input ambient_8h.wav --count-triggers
```

## Data layout produced
```
ai-core/asr/
  positives/   hey_osaima_0001.wav ...   (>= 50)
  negatives/   negative_0001.wav   ...   (2-3x positives)
  augmented/   (optional, generated)
  hey_osaima.onnx   (after training)
```
