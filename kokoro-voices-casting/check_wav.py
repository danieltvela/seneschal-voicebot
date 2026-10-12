import sys
import wave
import audioop
w = wave.open(sys.argv[1])
n, sw, fr = w.getnframes(), w.getsampwidth(), w.getframerate()
data = w.readframes(n)
rms = audioop.rms(data, sw)
print(f"{sys.argv[1].split('/')[-1]}: {n/fr:.1f}s @ {fr}Hz {sw*8}bit, RMS={rms} (silencio~0, voz sana >1000)")
