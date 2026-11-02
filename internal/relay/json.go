package relay

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"unicode/utf8"
)

var errInvalidFrame = errors.New("invalid relay frame")

// decodeFrame preserves exact field names and rejects JSON's otherwise accepted
// duplicate keys, null strings, invalid UTF-8 and concatenated values.
func decodeFrame(body []byte) (map[string]string, error) {
	if !utf8.Valid(body) {
		return nil, errInvalidFrame
	}
	dec := json.NewDecoder(bytes.NewReader(body))
	if token, err := dec.Token(); err != nil || token != json.Delim('{') {
		return nil, errInvalidFrame
	}
	fields := make(map[string]string, 8)
	for dec.More() {
		token, err := dec.Token()
		if err != nil {
			return nil, errInvalidFrame
		}
		key, ok := token.(string)
		if !ok || len(fields) >= 8 {
			return nil, errInvalidFrame
		}
		if _, exists := fields[key]; exists {
			return nil, errInvalidFrame
		}
		token, err = dec.Token()
		if err != nil {
			return nil, errInvalidFrame
		}
		value, ok := token.(string)
		if !ok {
			return nil, errInvalidFrame
		}
		fields[key] = value
	}
	if token, err := dec.Token(); err != nil || token != json.Delim('}') {
		return nil, errInvalidFrame
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return nil, errInvalidFrame
	}
	return fields, nil
}

func exactFields(fields map[string]string, kind string, names ...string) bool {
	if fields["type"] != kind || len(fields) != len(names)+1 {
		return false
	}
	for _, name := range names {
		if _, ok := fields[name]; !ok {
			return false
		}
	}
	return true
}

func decodeBase64(value string, max int) ([]byte, error) {
	if len(value) == 0 || len(value) > base64.StdEncoding.EncodedLen(max) {
		return nil, errInvalidFrame
	}
	decoded, err := base64.StdEncoding.Strict().DecodeString(value)
	if err != nil || len(decoded) == 0 || len(decoded) > max || base64.StdEncoding.EncodeToString(decoded) != value {
		return nil, errInvalidFrame
	}
	return decoded, nil
}

// A small, printable ASCII alphabet keeps identifiers unambiguous in envelopes.
func validRequestID(id string) bool {
	if len(id) < 1 || len(id) > 64 {
		return false
	}
	for _, c := range []byte(id) {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '_' || c == ':' || c == '-') {
			return false
		}
	}
	return true
}
