package httpapi

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
)

// parseStringObject deliberately rejects duplicate keys, nulls, unknown fields
// and multiple JSON values, which ordinary struct unmarshaling accepts.
func parseStringObject(body []byte, required []string) (map[string]string, error) {
	invalid := errors.New("invalid JSON object")
	decoder := json.NewDecoder(bytes.NewReader(body))
	token, err := decoder.Token()
	if err != nil || token != json.Delim('{') {
		return nil, invalid
	}
	allowed := make(map[string]bool, len(required))
	for _, field := range required {
		allowed[field] = true
	}
	fields := make(map[string]string, len(required))
	for decoder.More() {
		token, err := decoder.Token()
		if err != nil {
			return nil, invalid
		}
		key, ok := token.(string)
		if !ok || !allowed[key] {
			return nil, invalid
		}
		if _, duplicate := fields[key]; duplicate {
			return nil, invalid
		}
		token, err = decoder.Token()
		if err != nil {
			return nil, invalid
		}
		value, ok := token.(string)
		if !ok {
			return nil, invalid
		}
		fields[key] = value
	}
	if token, err = decoder.Token(); err != nil || token != json.Delim('}') || len(fields) != len(required) {
		return nil, invalid
	}
	if _, err = decoder.Token(); !errors.Is(err, io.EOF) {
		return nil, invalid
	}
	return fields, nil
}
