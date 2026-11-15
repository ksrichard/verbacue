"""Writes the reference output of coqui's text cleaning for Chinese, Japanese and Korean.

Usage: uv run --with pypinyin --with cutlet --with "fugashi[unidic-lite]" \
  romanize_fixtures.py <output dir>

XTTS-v2 reads these languages in Latin letters. Coqui converts them with pypinyin,
cutlet and hangul_romanize. The Rust code in crates/verbacue-dub/src/romanize.rs
does the same, and its tests compare against <output dir>/fixtures/romanize.json.
The lines have no digits, so the test checks only the conversion.
"""

import json
import sys
from pathlib import Path

from TTS.tts.layers.xtts.tokenizer import VoiceBpeTokenizer

LINES = {
    "zh": [
        "你好，你今天怎么样？",
        "我不知道他去哪里了。",
        "我们明天早上见吧！",
        "这个问题很重要。",
        "谢谢你的帮助，朋友。",
    ],
    "ja": [
        "こんにちは、お元気ですか？",
        "私は東京に住んでいます。",
        "ちょっと待ってください！",
        "明日は雨が降るでしょう。",
        "ありがとうございました。",
    ],
    "ko": [
        "안녕하세요, 잘 지내세요?",
        "저는 서울에 살고 있어요.",
        "잠깐만 기다려 주세요!",
        "물엿을 샀어요.",
        "감사합니다, 친구.",
    ],
}


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    tokenizer = VoiceBpeTokenizer()
    cases = [
        {"lang": lang, "text": text, "latin": tokenizer.preprocess_text(text, lang)}
        for lang, lines in LINES.items()
        for text in lines
    ]
    out = Path(sys.argv[1]) / "fixtures"
    out.mkdir(parents=True, exist_ok=True)
    (out / "romanize.json").write_text(json.dumps(cases, ensure_ascii=False, indent=1))
    for case in cases:
        print(case["lang"], case["text"], "->", case["latin"])


if __name__ == "__main__":
    main()
