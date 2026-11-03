package prekeys

import (
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/url"
	"strconv"
	"strings"
	"time"

	"void-nothingness/internal/account"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/queue"
)

type Service struct {
	store    Store
	keys     auth.KeyLookup
	audience string
	now      func() time.Time
}

func New(store Store, keys auth.KeyLookup, audience string) (*Service, error) {
	u, err := url.Parse(audience)
	if store == nil || keys == nil || err != nil || len(audience) > 2048 ||
		u.Scheme != "https" || u.Hostname() == "" || u.User != nil ||
		u.ForceQuery || u.RawQuery != "" || u.Fragment != "" ||
		strings.HasSuffix(audience, "/") || u.String() != audience || strings.ContainsAny(audience, "\r\n\t ") {
		return nil, ErrInvalid
	}
	return &Service{store: store, keys: keys, audience: audience, now: time.Now}, nil
}

// IdentityMessage is the exact Ed25519 account-binding transcript. The trailing
// newline is part of the signed bytes; the audience is the canonical API URL.
func IdentityMessage(audience, actor string, upload Upload) []byte {
	return []byte("VOID:signal-identity:v1\n" + audience + "\n" + actor + "\n1\n" +
		strconv.Itoa(upload.RegistrationID) + "\n" + upload.IdentityKey + "\n")
}

func (s *Service) Publish(ctx context.Context, actor, publicationID, payloadBase64 string) (Counts, error) {
	if !auth.ValidVoidID(actor) || !queue.ValidEventID(publicationID) {
		return Counts{}, ErrInvalid
	}
	upload, hash, err := ParseUpload(payloadBase64)
	if err != nil {
		return Counts{}, err
	}
	key, err := s.key(ctx, actor)
	if err != nil {
		return Counts{}, err
	}
	signature, _ := base64.StdEncoding.DecodeString(upload.AccountSignature)
	if !ed25519.Verify(ed25519.PublicKey(key), IdentityMessage(s.audience, actor, upload), signature) {
		return Counts{}, ErrInvalid
	}
	counts, err := s.store.Publish(ctx, actor, publicationID, upload, hash, s.now().UTC().Truncate(time.Microsecond))
	return counts, safeError(ctx, err)
}

func (s *Service) Claim(ctx context.Context, actor, requestID, target string) (string, error) {
	if !auth.ValidVoidID(actor) || !auth.ValidVoidID(target) || actor == target || !queue.ValidEventID(requestID) {
		return "", ErrInvalid
	}
	if _, err := s.key(ctx, actor); err != nil {
		return "", err
	}
	bundle, err := s.store.Claim(ctx, actor, requestID, target, s.now().UTC().Truncate(time.Microsecond))
	if err != nil {
		return "", safeError(ctx, err)
	}
	raw, err := json.Marshal(bundle)
	if err != nil || len(raw) > MaxBundleBytes {
		return "", ErrUnavailable
	}
	return base64.StdEncoding.EncodeToString(raw), nil
}

func (s *Service) Cleanup(ctx context.Context, limit int) (int64, error) {
	if limit < 1 || limit > 1000 {
		return 0, ErrInvalid
	}
	n, err := s.store.Expire(ctx, s.now().UTC().Truncate(time.Microsecond), limit)
	return n, safeError(ctx, err)
}

func (s *Service) Ready(ctx context.Context) error { return safeError(ctx, s.store.Validate(ctx)) }

func (s *Service) key(ctx context.Context, id string) ([]byte, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	number, _ := strconv.ParseInt(id, 10, 64)
	key, err := s.keys.AuthenticationKey(ctx, number)
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}
	if errors.Is(err, auth.ErrNotFound) {
		return nil, ErrInvalid
	}
	if err != nil || !account.ValidPublicKey(key) {
		return nil, ErrUnavailable
	}
	return key, nil
}

func safeError(ctx context.Context, err error) error {
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	for _, known := range []error{ErrInvalid, ErrConflict, ErrCapacity, ErrExpired, ErrUnavailable} {
		if errors.Is(err, known) {
			return known
		}
	}
	return ErrUnavailable
}
