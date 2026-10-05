package httptransport

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"unicode/utf8"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/account"
)

const maxAccountBodyBytes = 16 * 1024

type accountHandler struct {
	accounts *account.Service
}

type createAccountRequest struct {
	VOIDID            string `json:"void_id"`
	DisplayName       string `json:"display_name"`
	IdentityPublicKey string `json:"identity_public_key"`
}

func (handler accountHandler) create(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		methodNotAllowed(w, http.MethodPost)
		return
	}

	r.Body = http.MaxBytesReader(w, r.Body, maxAccountBodyBytes)
	body, err := io.ReadAll(r.Body)
	if err != nil {
		writeDecodeError(w, err)
		return
	}
	// encoding/json replaces invalid UTF-8; reject it instead of changing identity data.
	if !utf8.Valid(body) {
		writeError(w, http.StatusBadRequest, "request body must contain valid UTF-8")
		return
	}
	decoder := json.NewDecoder(bytes.NewReader(body))
	decoder.DisallowUnknownFields()
	var request *createAccountRequest
	if err := decoder.Decode(&request); err != nil {
		writeDecodeError(w, err)
		return
	}
	if request == nil {
		writeError(w, http.StatusBadRequest, "request body must be a JSON object")
		return
	}
	// Validate the whole body before creating anything, including trailing data.
	if err := decoder.Decode(new(any)); err != io.EOF {
		writeDecodeError(w, err)
		return
	}

	created, err := handler.accounts.Create(r.Context(), request.VOIDID, request.DisplayName, request.IdentityPublicKey)
	switch {
	case errors.Is(err, account.ErrInvalidAccount):
		writeError(w, http.StatusBadRequest, "void_id, display_name, and identity_public_key are required")
	case errors.Is(err, account.ErrAlreadyExists):
		writeError(w, http.StatusConflict, "void_id already exists")
	case err != nil:
		writeError(w, http.StatusInternalServerError, "internal server error")
	default:
		writeJSON(w, http.StatusCreated, created)
	}
}

func (handler accountHandler) get(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		methodNotAllowed(w, http.MethodGet)
		return
	}

	stored, err := handler.accounts.GetByVOIDID(r.Context(), r.PathValue("void_id"))
	switch {
	case errors.Is(err, account.ErrInvalidAccount):
		writeError(w, http.StatusBadRequest, "void_id is required")
	case errors.Is(err, account.ErrNotFound):
		writeError(w, http.StatusNotFound, "account not found")
	case err != nil:
		writeError(w, http.StatusInternalServerError, "internal server error")
	default:
		writeJSON(w, http.StatusOK, stored)
	}
}

func writeDecodeError(w http.ResponseWriter, err error) {
	var tooLarge *http.MaxBytesError
	if errors.As(err, &tooLarge) {
		writeError(w, http.StatusRequestEntityTooLarge, "request body too large")
		return
	}
	writeError(w, http.StatusBadRequest, "request body must be one valid JSON object with supported fields")
}

func methodNotAllowed(w http.ResponseWriter, allowed string) {
	w.Header().Set("Allow", allowed)
	writeError(w, http.StatusMethodNotAllowed, "method not allowed")
}

func writeError(w http.ResponseWriter, status int, message string) {
	writeJSON(w, status, struct {
		Error string `json:"error"`
	}{Error: message})
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}
