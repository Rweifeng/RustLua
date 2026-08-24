# One-off dev tool: synthesize a short looping chiptune background-music WAV master.
# Uses a compiled C# helper so sample synthesis runs at native speed.
# The shipped asset is bgm.ogg (convert bgm-master.wav via:
#   cargo run -p engine --example wav2ogg -- <master.wav> game/assets/audio/bgm.ogg)

param([string]$Out = "")

if ([string]::IsNullOrWhiteSpace($Out)) {
    $root = Split-Path -Parent $PSScriptRoot
    $Out = Join-Path $root "game\assets\audio\bgm-master.wav"
}

Add-Type -TypeDefinition @"
using System;
using System.IO;

public static class BgmGen {
    const int SampleRate = 22050;
    const int BeatsPerBar = 12;   // 4 bass beats + 16 melody half-beats

    static void WriteNote(BinaryWriter w, double freq, int lengthSamples, double amp) {
        for (int i = 0; i < lengthSamples; i++) {
            double t = (double)i / SampleRate;
            double v = Math.Sin(2 * Math.PI * freq * t) + 0.5 * Math.Sin(4 * Math.PI * freq * t);
            double env = Math.Min(1.0, i / (SampleRate * 0.05)) * (1.0 - 0.45 * ((double)i / lengthSamples));
            short s = (short)(((v / 1.5) * amp * env) * 32767);
            w.Write(s);
        }
    }

    public static void Generate(string path) {
        double[] bass = { 220.0, 349.23, 261.63, 392.00 };            // A3 F4 C4 G4
        double[] melody = { 440.0, 523.25, 659.25, 523.25, 349.23, 440.0, 523.25, 440.0,
                            329.63, 392.00, 523.25, 392.00, 392.00, 493.88, 587.33, 493.88 };
        int totalBars = 4;
        int beatSamples = SampleRate / 2;                             // 120 BPM
        int totalSamples = beatSamples * BeatsPerBar * totalBars;
        int dataSize = totalSamples * 2;

        using (var stream = new MemoryStream()) {
            var w = new BinaryWriter(stream);
            w.Write(System.Text.Encoding.ASCII.GetBytes("RIFF"));
            w.Write(36 + dataSize);
            w.Write(System.Text.Encoding.ASCII.GetBytes("WAVE"));
            w.Write(System.Text.Encoding.ASCII.GetBytes("fmt "));
            w.Write(16); w.Write((short)1); w.Write((short)1);
            w.Write(SampleRate); w.Write(SampleRate * 2);
            w.Write((short)2); w.Write((short)16);
            w.Write(System.Text.Encoding.ASCII.GetBytes("data"));
            w.Write(dataSize);

            for (int bar = 0; bar < totalBars; bar++) {
                for (int beat = 0; beat < 4; beat++) {
                    WriteNote(w, bass[bar % bass.Length] * 1.0, beatSamples, 0.30);
                }
                for (int i = 0; i < 16; i++) {
                    int noteLen = beatSamples / 2;
                    WriteNote(w, melody[(bar * 16 + i) % melody.Length], noteLen, 0.16);
                }
            }
            w.Flush();
            File.WriteAllBytes(path, stream.ToArray());
        }
    }
}
"@

[BgmGen]::Generate($Out)
Write-Output "bgm wav -> $Out"
