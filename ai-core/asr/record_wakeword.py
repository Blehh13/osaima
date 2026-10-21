#!/usr/bin/env python3
"""
OSAIMA wake-word data capture (PDR §5.3 step 12): collect "Hey OSAIMA" samples
to train a custom openWakeWord model. Minimum 50 positive samples.

I (the assistant) cannot record audio for you — this is the tool you run at your
mic. It captures fixed-length WAV clips at 16 kHz mono (openWakeWord's expected
format) into positives/ and negatives/.

    pip install sounddevice soundfile numpy
    python record_wakeword.py --label positive --count 60   # say "Hey OSAIMA"
    python record_wakeword.py --label negative --count 120  # say anything else / silence

Then train with openWakeWord's training pipeline (see WAKEWORD-GUIDE.md).
"""
import argparse, os, sys, time

SR = 16000          # openWakeWord expects 16 kHz mono
DUR = 2.0           # seconds per clip

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--label", choices=["positive", "negative"], required=True)
    ap.add_argument("--count", type=int, default=50)
    ap.add_argument("--outdir", default=os.path.dirname(os.path.abspath(__file__)))
    ap.add_argument("--duration", type=float, default=DUR)
    args = ap.parse_args()

    try:
        import sounddevice as sd, soundfile as sf, numpy as np
    except ImportError:
        sys.exit("Install deps first:  pip install sounddevice soundfile numpy")

    folder = os.path.join(args.outdir, "positives" if args.label == "positive" else "negatives")
    os.makedirs(folder, exist_ok=True)
    existing = len([f for f in os.listdir(folder) if f.endswith(".wav")])

    prompt = ('Say "Hey OSAIMA" clearly' if args.label == "positive"
              else "Say anything else, background noise, or stay silent")
    print(f"Capturing {args.count} {args.label} clips ({args.duration}s @ {SR}Hz mono) into {folder}")
    print(f"{prompt}. Ctrl+C to stop.\n")

    for i in range(args.count):
        input(f"[{i+1}/{args.count}] press Enter, then speak...")
        rec = sd.rec(int(args.duration * SR), samplerate=SR, channels=1, dtype="int16")
        sd.wait()
        idx = existing + i + 1
        path = os.path.join(folder, f"{args.label}_{idx:04d}.wav")
        sf.write(path, rec, SR, subtype="PCM_16")
        print(f"    saved {os.path.basename(path)}")

    total = len([f for f in os.listdir(folder) if f.endswith('.wav')])
    print(f"\nDone. {folder} now holds {total} clips.")
    if args.label == "positive" and total < 50:
        print(f"WARNING: {total} positives — capture at least 50 for a usable model.")

if __name__ == "__main__":
    main()
