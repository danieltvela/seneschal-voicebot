# src/profile/ — User Profile Facts Extraction

## Responsibility

Extract **user profile facts** (name, age, city, job, hobbies, etc.) from conversation exchanges via LLM analysis. Also detect **user corrections** (immutable rules) via rule-based pattern matching. Builds the `[USER PROFILE]` and `[IMMUTABLE RULES]` blocks injected into the system prompt.

## Design

### Core Structures

- **`ProfileFact`** — `key: String`, `value: String`, `confidence: f64`.
- **`Correction`** — `topic: String`, `correction_text: String`, `confidence: f64`.
- **`RawFact`** — Deserialized LLM response: `key`, `value`, `confidence` (defaults to 0.8).

### Constants

- `MIN_INJECT_CONFIDENCE = 0.5` — Minimum confidence threshold for profile facts to be injected into the system prompt.

### System Prompt Injection

`build_profile_context(facts)` filters facts by confidence and formats as:
```
[USER PROFILE]
key: value
key: value
```
Returns empty string if no facts meet the threshold.

`build_corrections_context(corrections)` formats corrections as:
```
[IMMUTABLE RULES]
- The user corrected me: topic -> correction_text
```

### Correction Detection (Rule-Based)

`detect_corrections(user_text, assistant_text)` uses **substring matching** against predefined trigger patterns in both Spanish and English:

**Spanish triggers**: `no, en realidad`, `no es así`, `me equivoqué`, `corrijo`, `error`, `incorrecto`, `deberías saber`, `no me gusta`, `no es correcto`

**English triggers**: `no, actually`, `that's not right`, `i was wrong`, `correction`, `i made a mistake`, `you're wrong`, `that's incorrect`, `not correct`, `wrong about`

When a trigger matches:
- `extract_correction_topic` — Finds topic after `sobre ` (ES) or `about ` (EN); defaults to `"general"`.
- `extract_correction_clause` — Extracts text after the trigger, clipped to ~120 characters (splitting at sentence boundary if possible).
- Returns `Correction { topic, correction_text, confidence: 1.0 }`.

## Flow

### Profile Context Building

```
build_profile_context(facts: &[ProfileFact])
  → Filter: facts where confidence >= 0.5
  → If empty: return ""
  → Format: "\n\n[USER PROFILE]\n" + "{key}: {value}\n" for each fact
```

### Fact Extraction

```
extract_facts(client, user_text, assistant_text)
  → LLM prompt:
    system: "Extract facts about the user... Return JSON array... Keys: name, age, city, country, language, job, company, field, skill, hobby, pet, family, preference, communication_style, personality_trait"
    user: "User: {user_text}\nAssistant: {assistant_text}"
  
  → client.complete_short(messages) → raw JSON string
  → parse_facts(raw) → Vec<ProfileFact>

parse_facts(raw):
  → strip_code_fence(raw) → strip ```json and ``` markers
  → serde_json::from_str<Vec<RawFact>>()
  
  → For each fact:
    filter: key and value must be non-empty
    normalize_key(key): lowercase, spaces→underscores, alphanumeric only
    clamp confidence to [0.0, 1.0]
    default confidence = 0.8 if missing
  
  → Return Vec<ProfileFact>
```

### Correction Detection

```
detect_corrections(user_text, assistant_text)
  → text_lower = user_text.to_lowercase()
  
  → For each trigger in (es_patterns + en_patterns):
    if text_lower.contains(trigger):
      topic = extract_correction_topic(text_lower)
      correction_text = extract_correction_clause(user_text, trigger)
      push Correction { topic, correction_text, confidence: 1.0 }
      break (first match only)
  
  → Return Vec<Correction>

extract_correction_topic(text_lower):
  → Find "sobre " or "about " → next word → topic
  → Default: "general"

extract_correction_clause(original, trigger):
  → Find trigger in original (case-insensitive)
  → Extract text after trigger
  → If > 120 chars: clip at last sentence boundary (., !, ?) or hard clip at 120
  → Trim and return
```

### Corrections Context Building

```
build_corrections_context(corrections: &[Correction])
  → If empty: return ""
  → Format: "\n\n[IMMUTABLE RULES]\n" + "- The user corrected me: {topic} -> {text}\n"
```

## Integration

### Dependencies
- `crate::llm::LlmProvider` — LLM client for fact extraction.
- `crate::llm::Message` — Message construction.
- `serde::Deserialize` — JSON deserialization.
- `tracing` — Logging.

### Consumers
- `src/dream/` — Calls `extract_facts` during consolidation cycles; calls `detect_corrections` to find immutable rules.
- `src/daemon.rs` / `src/llm/` — Calls `build_profile_context` and `build_corrections_context` to inject into system prompt.
- `src/db/` — Stores facts via `upsert_profile_fact`; corrections stored with `correction:` prefix.

### Data Flow
```
S-DREAM cycle
  → extract_facts(client, user_text, conversation_text)
  → for fact in facts: upsert_profile_fact(fact.key, fact.value, fact.confidence)
  
  → detect_corrections(session)
  → for correction: upsert_profile_fact("correction:{topic}", correction_text, 1.0)

Startup / system prompt building
  → load_user_profile() from Database
  → build_profile_context(facts) → inject into system prompt
  → get_immutable_rules() from Database
  → build_corrections_context(corrections) → inject into system prompt
```