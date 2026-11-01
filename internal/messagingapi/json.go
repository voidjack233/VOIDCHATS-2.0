package messagingapi

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"mime"
	"net/http"
	"reflect"
	"strings"
	"unicode/utf8"
)

// Validate every object before unmarshalling: encoding/json otherwise accepts
// duplicate keys and null scalars. The second pass enforces each typed schema.
func strictJSON(body []byte, out any) error {
	if !utf8.Valid(body) {
		return errors.New("invalid UTF-8")
	}
	d := json.NewDecoder(bytes.NewReader(body))
	budget := 2048
	if err := jsonValue(d, 0, &budget); err != nil {
		return err
	}
	if _, err := d.Token(); !errors.Is(err, io.EOF) {
		return errors.New("trailing JSON")
	}
	var shape any
	if err := json.Unmarshal(body, &shape); err != nil {
		return err
	}
	if err := exactShape(shape, reflect.TypeOf(out)); err != nil {
		return err
	}
	d = json.NewDecoder(bytes.NewReader(body))
	d.DisallowUnknownFields()
	return d.Decode(out)
}

func exactShape(value any, typ reflect.Type) error {
	if typ.Kind() == reflect.Pointer {
		typ = typ.Elem()
	}
	switch typ.Kind() {
	case reflect.Struct:
		object, ok := value.(map[string]any)
		if !ok {
			return errors.New("expected JSON object")
		}
		fields := make(map[string]reflect.Type)
		for i := 0; i < typ.NumField(); i++ {
			field := typ.Field(i)
			name := strings.Split(field.Tag.Get("json"), ",")[0]
			if name == "-" || field.PkgPath != "" {
				continue
			}
			if name == "" {
				name = field.Name
			}
			fields[name] = field.Type
		}
		if len(object) != len(fields) {
			return errors.New("missing or unknown field")
		}
		for name, item := range object {
			fieldType, ok := fields[name]
			if !ok {
				return errors.New("unknown field")
			}
			if err := exactShape(item, fieldType); err != nil {
				return err
			}
		}
	case reflect.Slice:
		items, ok := value.([]any)
		if !ok {
			return errors.New("expected JSON array")
		}
		for _, item := range items {
			if err := exactShape(item, typ.Elem()); err != nil {
				return err
			}
		}
	case reflect.String:
		if _, ok := value.(string); !ok {
			return errors.New("expected JSON string")
		}
	default:
		return errors.New("unsupported request schema")
	}
	return nil
}

func jsonValue(d *json.Decoder, depth int, budget *int) error {
	*budget--
	if depth > 8 || *budget < 0 {
		return errors.New("JSON nesting too deep")
	}
	t, err := d.Token()
	if err != nil {
		return err
	}
	if t == nil {
		return errors.New("null is not accepted")
	}
	delim, composite := t.(json.Delim)
	if !composite {
		return nil
	}
	switch delim {
	case '{':
		seen := make(map[string]bool)
		for d.More() {
			if len(seen) >= 32 {
				return errors.New("too many JSON fields")
			}
			key, err := d.Token()
			if err != nil {
				return err
			}
			name, ok := key.(string)
			if !ok || seen[name] {
				return errors.New("duplicate or invalid field")
			}
			seen[name] = true
			if err := jsonValue(d, depth+1, budget); err != nil {
				return err
			}
		}
	case '[':
		count := 0
		for d.More() {
			count++
			if count > 100 {
				return errors.New("too many JSON array items")
			}
			if err := jsonValue(d, depth+1, budget); err != nil {
				return err
			}
		}
	default:
		return errors.New("invalid JSON delimiter")
	}
	end, err := d.Token()
	if err != nil || (delim == '{' && end != json.Delim('}')) || (delim == '[' && end != json.Delim(']')) {
		return errors.New("invalid JSON closure")
	}
	return nil
}

func readBody(w http.ResponseWriter, r *http.Request, max int64, out any) bool {
	media, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if err != nil || media != "application/json" {
		writeError(w, 415, "unsupported_media_type", "Content-Type must be application/json.")
		return false
	}
	if r.ContentLength > max {
		writeError(w, 413, "body_too_large", "Request body exceeds its limit.")
		return false
	}
	if deadline, ok := r.Context().Deadline(); ok {
		if err := http.NewResponseController(w).SetReadDeadline(deadline); err != nil && !errors.Is(err, http.ErrNotSupported) {
			writeError(w, 503, "unavailable", "Messaging is temporarily unavailable.")
			return false
		}
	}
	r.Body = http.MaxBytesReader(w, r.Body, max)
	defer r.Body.Close()
	body, err := io.ReadAll(r.Body)
	if err != nil {
		var tooLarge *http.MaxBytesError
		switch {
		case errors.As(err, &tooLarge):
			writeError(w, 413, "body_too_large", "Request body exceeds its limit.")
		case r.Context().Err() != nil:
			writeError(w, 504, "deadline_exceeded", "Request deadline exceeded.")
		default:
			var timeout interface{ Timeout() bool }
			if errors.As(err, &timeout) && timeout.Timeout() {
				writeError(w, 504, "deadline_exceeded", "Request deadline exceeded.")
			} else {
				writeError(w, 400, "invalid_json", "Invalid JSON request.")
			}
		}
		return false
	}
	if len(bytes.TrimSpace(body)) == 0 || bytes.TrimSpace(body)[0] != '{' || strictJSON(body, out) != nil {
		writeError(w, 400, "invalid_json", "Invalid JSON request.")
		return false
	}
	return true
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	data, err := json.Marshal(value)
	if err != nil {
		writeError(w, 503, "unavailable", "Messaging is temporarily unavailable.")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_, _ = w.Write(append(data, '\n'))
}

func writeError(w http.ResponseWriter, status int, code, message string) {
	if status == 401 {
		w.Header().Set("WWW-Authenticate", "Bearer")
	}
	if status == 429 || status == 503 {
		if w.Header().Get("Retry-After") == "" {
			w.Header().Set("Retry-After", "5")
		}
	}
	writeJSON(w, status, map[string]any{"error": map[string]string{"code": code, "message": message}})
}
