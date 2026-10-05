package voidid

import (
	"bytes"
	"encoding"
	"encoding/json"
	"errors"
	"io"
	"strings"
	"testing"
)

var (
	_ encoding.TextMarshaler   = ID{}
	_ encoding.TextUnmarshaler = (*ID)(nil)
)

func TestNewValidAndNoSampleCollisions(t *testing.T) {
	// A sample detects implementation mistakes such as constant or truncated
	// randomness. It is not a proof of global uniqueness; storage must enforce it.
	const samples = 10_000
	seen := make(map[ID]struct{}, samples)
	for i := 0; i < samples; i++ {
		id, err := New()
		if err != nil {
			t.Fatalf("New() sample %d: %v", i, err)
		}
		if !id.IsValid() {
			t.Fatalf("New() sample %d returned an invalid ID", i)
		}
		if err := Validate(id.String()); err != nil {
			t.Fatalf("Validate() sample %d: %v", i, err)
		}
		parsed, err := Parse(id.String())
		if err != nil || parsed != id {
			t.Fatalf("round trip sample %d: got %v, error %v", i, parsed, err)
		}
		if _, found := seen[id]; found {
			t.Fatalf("duplicate generated ID at sample %d", i)
		}
		seen[id] = struct{}{}
	}
}

func TestKnownEncodingVectors(t *testing.T) {
	tests := []struct {
		name string
		data []byte
		text string
	}{
		{
			name: "zero bits remain a valid generated ID",
			data: make([]byte, entropyBytes),
			text: "VOID-AAAAAAAA-AAAAAAAA-AAAAAAAA-AAAAAAAA",
		},
		{
			name: "all bits set",
			data: bytes.Repeat([]byte{0xff}, entropyBytes),
			text: "VOID-77777777-77777777-77777777-77777777",
		},
		{
			name: "sequential bytes",
			data: []byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19},
			text: "VOID-AAAQEAYE-AUDAOCAJ-BIFQYDIO-B4IBCEQT",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			generated, err := generate(bytes.NewReader(tt.data))
			if err != nil {
				t.Fatalf("generate(): %v", err)
			}
			if got := generated.String(); got != tt.text {
				t.Fatalf("String() = %q, want %q", got, tt.text)
			}
			parsed, err := Parse(tt.text)
			if err != nil {
				t.Fatalf("Parse(): %v", err)
			}
			if parsed != generated || !parsed.IsValid() {
				t.Fatal("parsed and generated IDs must be valid and equal")
			}
			if parsed == (ID{}) {
				t.Fatal("valid ID must differ from the invalid zero value")
			}
		})
	}
}

func TestRejectMalformedText(t *testing.T) {
	const valid = "VOID-AAAQEAYE-AUDAOCAJ-BIFQYDIO-B4IBCEQT"
	tests := map[string]string{
		"empty":               "",
		"truncated":           valid[:len(valid)-1],
		"missing group":       valid[:31],
		"too long":            valid + "A",
		"leading whitespace":  " " + valid,
		"trailing whitespace": valid + " ",
		"lowercase":           strings.ToLower(valid),
		"lowercase payload":   "VOID-" + strings.ToLower(valid[5:]),
		"wrong prefix":        "VOIP" + valid[4:],
		"missing separators":  strings.ReplaceAll(valid, "-", ""),
		"wrong separator":     strings.Replace(valid, "-", "_", 2),
		"misplaced separator": valid[:12] + "-A" + valid[14:],
		"digit zero":          valid[:5] + "0" + valid[6:],
		"digit one":           valid[:5] + "1" + valid[6:],
		"digit eight":         valid[:5] + "8" + valid[6:],
		"padding":             valid[:39] + "=",
		"newline":             valid[:5] + "\n" + valid[6:],
		"carriage return":     valid[:5] + "\r" + valid[6:],
		"NUL":                 valid[:5] + "\x00" + valid[6:],
		"non-ASCII":           valid[:5] + "Ａ" + valid[8:],
	}
	for name, text := range tests {
		t.Run(name, func(t *testing.T) {
			id, err := Parse(text)
			if !errors.Is(err, ErrInvalid) {
				t.Fatalf("Parse(%q) error = %v, want ErrInvalid", text, err)
			}
			if id.IsValid() || id != (ID{}) {
				t.Fatal("failed Parse() must return the invalid zero value")
			}
			if err := Validate(text); !errors.Is(err, ErrInvalid) {
				t.Fatalf("Validate(%q) error = %v, want ErrInvalid", text, err)
			}
		})
	}
}

func TestZeroValueAndTextSerialization(t *testing.T) {
	var zero ID
	if zero.IsValid() || zero.String() != "" {
		t.Fatal("the zero value must be invalid and have no text representation")
	}
	if text, err := zero.MarshalText(); !errors.Is(err, ErrInvalid) || text != nil {
		t.Fatalf("zero.MarshalText() = %q, %v; want nil, ErrInvalid", text, err)
	}
	if _, err := json.Marshal(zero); !errors.Is(err, ErrInvalid) {
		t.Fatalf("json.Marshal(zero) error = %v, want ErrInvalid", err)
	}
	if err := zero.UnmarshalText([]byte("invalid")); !errors.Is(err, ErrInvalid) {
		t.Fatalf("zero.UnmarshalText() error = %v, want ErrInvalid", err)
	}
	if zero != (ID{}) || zero.IsValid() {
		t.Fatal("failed UnmarshalText() on a fresh value must leave it invalid")
	}

	const canonical = "VOID-AAAQEAYE-AUDAOCAJ-BIFQYDIO-B4IBCEQT"
	var id ID
	if err := id.UnmarshalText([]byte(canonical)); err != nil {
		t.Fatalf("UnmarshalText(): %v", err)
	}
	text, err := id.MarshalText()
	if err != nil || string(text) != canonical {
		t.Fatalf("MarshalText() = %q, %v; want %q, nil", text, err, canonical)
	}
	previous := id
	if err := id.UnmarshalText([]byte("invalid")); !errors.Is(err, ErrInvalid) {
		t.Fatalf("malformed UnmarshalText() error = %v, want ErrInvalid", err)
	}
	if id != previous {
		t.Fatal("failed UnmarshalText() must preserve the previous value")
	}
	var nilReceiver *ID
	if err := nilReceiver.UnmarshalText([]byte(canonical)); !errors.Is(err, ErrInvalid) {
		t.Fatalf("nil receiver UnmarshalText() error = %v, want ErrInvalid", err)
	}

	encoded, err := json.Marshal(id)
	if err != nil {
		t.Fatalf("json.Marshal(): %v", err)
	}
	if string(encoded) != `"`+canonical+`"` {
		t.Fatalf("JSON representation = %s, want a canonical string", encoded)
	}
	var decoded ID
	if err := json.Unmarshal(encoded, &decoded); err != nil || decoded != id {
		t.Fatalf("JSON round trip = %v, %v; want %v, nil", decoded, err, id)
	}
	if err := json.Unmarshal([]byte(`"invalid"`), &decoded); !errors.Is(err, ErrInvalid) {
		t.Fatalf("malformed JSON string error = %v, want ErrInvalid", err)
	}
}

func TestGenerateRejectsRandomReadFailures(t *testing.T) {
	readFailure := errors.New("secure random source unavailable")
	tests := []struct {
		name   string
		reader io.Reader
		want   error
	}{
		{name: "empty source", reader: bytes.NewReader(nil), want: io.EOF},
		{name: "short source", reader: bytes.NewReader(make([]byte, entropyBytes-1)), want: io.ErrUnexpectedEOF},
		{name: "source error", reader: failingReader{err: readFailure}, want: readFailure},
		{
			name:   "source error after partial read",
			reader: io.MultiReader(bytes.NewReader([]byte{1, 2, 3}), failingReader{err: readFailure}),
			want:   readFailure,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			id, err := generate(tt.reader)
			if !errors.Is(err, tt.want) {
				t.Fatalf("generate() error = %v, want %v", err, tt.want)
			}
			if id.IsValid() || id != (ID{}) {
				t.Fatal("failed generation must return the invalid zero value")
			}
		})
	}
}

func TestGenerateReadsCompleteEntropyAcrossShortReads(t *testing.T) {
	reader := &oneByteReader{data: bytes.Repeat([]byte{0xff}, entropyBytes)}
	id, err := generate(reader)
	if err != nil {
		t.Fatalf("generate(): %v", err)
	}
	if got := id.String(); got != "VOID-77777777-77777777-77777777-77777777" {
		t.Fatalf("String() = %q; short reads must retain all 160 bits", got)
	}
}

type failingReader struct{ err error }

func (r failingReader) Read([]byte) (int, error) { return 0, r.err }

type oneByteReader struct{ data []byte }

func (r *oneByteReader) Read(p []byte) (int, error) {
	if len(r.data) == 0 {
		return 0, io.EOF
	}
	p[0] = r.data[0]
	r.data = r.data[1:]
	return 1, nil
}
