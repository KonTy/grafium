import packageHelper from "../../../scripts/prepare-reader-voice.py?raw";

export const VOICE_SETUP_URLS = {
  catalog: "https://rhasspy.github.io/piper-samples/",
  piperDocs: "https://github.com/OHF-Voice/piper1-gpl/blob/main/docs/VOICES.md",
  linuxFiles: "https://huggingface.co/rhasspy/piper-voices/tree/main/en/en_US/ljspeech/high",
  linuxModel: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/ljspeech/high/en_US-ljspeech-high.onnx?download=true",
  linuxConfig: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/ljspeech/high/en_US-ljspeech-high.onnx.json?download=true",
  modelCard: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/ljspeech/high/MODEL_CARD?download=true",
  modelLicense: "https://brycebeattie.com/files/tts/",
  datasetLicense: "https://keithito.com/LJ-Speech-Dataset/#license",
  androidModel: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-ljspeech-high.tar.bz2",
  androidDocs: "https://k2-fsa.github.io/sherpa/onnx/tts/all/English/vits-piper-en_US-ljspeech-high.html",
  androidCatalog: "https://k2-fsa.github.io/sherpa/onnx/tts/pretrained_models/index.html",
} as const;

export const PIPER_SETUP_COMMAND = '/usr/bin/python3 -m venv "$HOME/.local/share/grafium-piper"\n'
  + '"$HOME/.local/share/grafium-piper/bin/pip" install "piper-tts==1.8.0"';

export function voicePackageCommand(android: boolean): string {
  const runtime = android ? "sherpa-vits-v1" : "piper-onnx-v1";
  return `python3 - . --runtime ${runtime} --id en_US-ljspeech-high `
    + `--name "LJ Speech (US English, high)" --language en-US --sample-rate 22050 `
    + `--license "Public domain model; dataset public domain in US" `
    + `--license-url ${VOICE_SETUP_URLS.modelLicense} <<'GRAFIUM_VOICE_PY'\n`
    + packageHelper.trimEnd() + "\nGRAFIUM_VOICE_PY";
}
