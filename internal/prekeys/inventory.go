package prekeys

import (
	"encoding/hex"
	"encoding/json"
	"sort"
	"time"

	"void-nothingness/internal/queue"
)

// ApplyPublication is shared by the transactional stores. It returns an
// independent inventory and never modifies current, including on failure.
func ApplyPublication(current *Inventory, publicationID string, upload Upload, hash [32]byte, now time.Time) (Inventory, Counts, error) {
	if !queue.ValidEventID(publicationID) || validateUpload(upload) != nil {
		return Inventory{}, Counts{}, ErrInvalid
	}
	digest := hex.EncodeToString(hash[:])
	var next Inventory
	if current == nil {
		next = Inventory{Upload: upload, Publications: map[string]string{}, CreatedAt: now}
		next.Upload.OneTimePrekeys = []PublicKey{}
		next.Upload.OneTimeKyberPrekeys = []SignedKey{}
	} else {
		next = cloneInventory(*current)
		if oldHash, exists := next.Publications[publicationID]; exists {
			if oldHash != digest {
				return Inventory{}, Counts{}, ErrConflict
			}
			return next, inventoryCounts(next), nil
		}
		if !sameIdentity(next.Upload, upload) {
			return Inventory{}, Counts{}, ErrConflict
		}
	}
	if len(next.Publications) >= MaxPublications ||
		len(next.Upload.OneTimePrekeys)+len(upload.OneTimePrekeys) > MaxInventoryKeys ||
		len(next.Upload.OneTimeKyberPrekeys)+len(upload.OneTimeKyberPrekeys) > MaxInventoryKeys {
		return Inventory{}, Counts{}, ErrCapacity
	}
	maxEC, maxKyber := next.MaxECID, next.MaxKyberID
	for _, key := range upload.OneTimePrekeys {
		if key.KeyID <= next.MaxECID {
			return Inventory{}, Counts{}, ErrConflict
		}
		maxEC = max(maxEC, key.KeyID)
	}
	for _, key := range upload.OneTimeKyberPrekeys {
		if key.KeyID <= next.MaxKyberID {
			return Inventory{}, Counts{}, ErrConflict
		}
		maxKyber = max(maxKyber, key.KeyID)
	}
	next.Upload.OneTimePrekeys = append(next.Upload.OneTimePrekeys, upload.OneTimePrekeys...)
	next.Upload.OneTimeKyberPrekeys = append(next.Upload.OneTimeKyberPrekeys, upload.OneTimeKyberPrekeys...)
	sort.Slice(next.Upload.OneTimePrekeys, func(i, j int) bool { return next.Upload.OneTimePrekeys[i].KeyID < next.Upload.OneTimePrekeys[j].KeyID })
	sort.Slice(next.Upload.OneTimeKyberPrekeys, func(i, j int) bool {
		return next.Upload.OneTimeKyberPrekeys[i].KeyID < next.Upload.OneTimeKyberPrekeys[j].KeyID
	})
	next.MaxECID, next.MaxKyberID = maxEC, maxKyber
	next.Publications[publicationID] = digest
	next.UpdatedAt = now
	return next, inventoryCounts(next), nil
}

// SelectBundle consumes at most one key from each one-time inventory, falling
// back to the reusable Kyber public key. Retrying belongs to the store's claim
// transaction, which persists the returned bundle before committing inventory.
func SelectBundle(current Inventory, now time.Time) (Inventory, Bundle, error) {
	if validateUpload(current.Upload) != nil {
		return Inventory{}, Bundle{}, ErrUnavailable
	}
	next := cloneInventory(current)
	upload := next.Upload
	bundle := Bundle{
		V: upload.V, DeviceID: upload.DeviceID, RegistrationID: upload.RegistrationID,
		IdentityKey: upload.IdentityKey, AccountSignature: upload.AccountSignature,
		SignedPrekey:   upload.SignedPrekey,
		KyberPrekey:    ClaimedKyberKey{SignedKey: upload.KyberLastResort, LastResort: true},
		ClaimExpiresAt: now.UTC().Truncate(time.Microsecond).Add(ClaimTTL),
	}
	if len(upload.OneTimePrekeys) > 0 {
		key := upload.OneTimePrekeys[0]
		bundle.OneTimePrekey = &key
		next.Upload.OneTimePrekeys = append([]PublicKey{}, upload.OneTimePrekeys[1:]...)
	}
	if len(upload.OneTimeKyberPrekeys) > 0 {
		bundle.KyberPrekey = ClaimedKyberKey{SignedKey: upload.OneTimeKyberPrekeys[0], LastResort: false}
		next.Upload.OneTimeKyberPrekeys = append([]SignedKey{}, upload.OneTimeKyberPrekeys[1:]...)
	}
	raw, err := json.Marshal(bundle)
	if err != nil || len(raw) > MaxBundleBytes {
		return Inventory{}, Bundle{}, ErrUnavailable
	}
	next.UpdatedAt = now
	return next, bundle, nil
}

func sameIdentity(a, b Upload) bool {
	return a.V == b.V && a.DeviceID == b.DeviceID && a.RegistrationID == b.RegistrationID &&
		a.IdentityKey == b.IdentityKey && a.AccountSignature == b.AccountSignature &&
		a.SignedPrekey == b.SignedPrekey && a.KyberLastResort == b.KyberLastResort
}

func inventoryCounts(inventory Inventory) Counts {
	return Counts{ECRemaining: len(inventory.Upload.OneTimePrekeys), KyberRemaining: len(inventory.Upload.OneTimeKyberPrekeys)}
}

func cloneInventory(inventory Inventory) Inventory {
	clone := inventory
	clone.Upload.OneTimePrekeys = append([]PublicKey{}, inventory.Upload.OneTimePrekeys...)
	clone.Upload.OneTimeKyberPrekeys = append([]SignedKey{}, inventory.Upload.OneTimeKyberPrekeys...)
	clone.Publications = make(map[string]string, len(inventory.Publications))
	for id, hash := range inventory.Publications {
		clone.Publications[id] = hash
	}
	return clone
}
