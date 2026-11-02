// Package blackbox verifies the real server executable and a separately
// provisioned, disposable PostgreSQL cluster. It never provisions schema or
// reads a deployment environment file.
package blackbox

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"void-nothingness/internal/account"
	"void-nothingness/internal/messaging"
	"void-nothingness/internal/session"
)

const apiPrefix = "/dm9pZG5vdGhpbmduZXNz"
const testAudience = "https://api.voidchats.xyz" + apiPrefix

type isolatedState struct {
	Version     int    `json:"version"`
	RootDir     string `json:"root_dir"`
	DataDir     string `json:"data_dir"`
	SocketDir   string `json:"socket_dir"`
	Port        int    `json:"port"`
	Database    string `json:"database"`
	RuntimeRole string `json:"runtime_role"`
	AdminRole   string `json:"admin_role"`
	AppURL      string `json:"app_url"`
	AdminURL    string `json:"admin_url"`
	PGCtl       string `json:"pg_ctl"`
	ServerPID   int    `json:"server_pid"`
}

func ownedPrivatePath(t *testing.T, path string, directory bool) {
	t.Helper()
	info, err := os.Lstat(path)
	if err != nil || info.Mode()&os.ModeSymlink != 0 || directory != info.IsDir() || (!directory && !info.Mode().IsRegular()) {
		t.Fatal("isolated cluster guard: required private path is absent or has an unsafe type")
	}
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok || stat.Uid != uint32(os.Getuid()) || info.Mode().Perm()&0077 != 0 {
		t.Fatal("isolated cluster guard: paths must belong to the test UID and have no group/other access")
	}
}

func within(root, child string) bool {
	rel, err := filepath.Rel(root, child)
	return err == nil && rel != "." && rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator))
}

func loadIsolatedState(t *testing.T) isolatedState {
	t.Helper()
	if os.Getenv("VOID_RUN_MESSAGING_BLACKBOX") != "1" {
		t.Skip("real-process messaging verification disabled; explicit opt-in and an owner-provisioned private disposable cluster are required")
	}
	statePath := os.Getenv("VOID_MESSAGING_BLACKBOX_STATE")
	if !filepath.IsAbs(statePath) {
		t.Fatal("set VOID_MESSAGING_BLACKBOX_STATE to the absolute private isolated cluster state path")
	}
	ownedPrivatePath(t, statePath, false)
	data, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal("cannot read isolated cluster descriptor")
	}
	var s isolatedState
	if json.Unmarshal(data, &s) != nil || s.Version != 1 || !strings.HasPrefix(s.Database, "void_test_") || s.Database == "void_test_" || s.RuntimeRole == "" || s.AdminRole == "" || s.Port < 1 || s.Port > 65535 {
		t.Fatal("isolated cluster guard: invalid descriptor or non-disposable database")
	}
	for _, path := range []string{s.RootDir, s.DataDir, s.SocketDir} {
		if !filepath.IsAbs(path) || filepath.Clean(path) != path {
			t.Fatal("isolated cluster guard: all paths must be canonical absolute paths")
		}
		resolved, err := filepath.EvalSymlinks(path)
		if err != nil || resolved != path {
			t.Fatal("isolated cluster guard: cluster paths must not traverse symlinks")
		}
		ownedPrivatePath(t, path, true)
	}
	if !within(s.RootDir, statePath) || !within(s.RootDir, s.DataDir) || !within(s.RootDir, s.SocketDir) {
		t.Fatal("isolated cluster guard: descriptor, data and socket must share one private root")
	}
	marker := filepath.Join(s.RootDir, ".void-isolated-messaging-cluster")
	ownedPrivatePath(t, marker, false)
	markedDatabase, err := os.ReadFile(marker)
	if err != nil || strings.TrimSpace(string(markedDatabase)) != s.Database {
		t.Fatal("isolated cluster guard: explicit disposable-cluster ownership marker does not match")
	}
	if !filepath.IsAbs(s.PGCtl) || filepath.Base(s.PGCtl) != "pg_ctl" {
		t.Fatal("isolated cluster guard: absolute pg_ctl executable required")
	}
	for _, entry := range []struct{ dsn, role string }{{s.AppURL, s.RuntimeRole}, {s.AdminURL, s.AdminRole}} {
		cfg, err := pgx.ParseConfig(entry.dsn)
		if err != nil || cfg.Database != s.Database || cfg.User != entry.role || cfg.Host != s.SocketDir || int(cfg.Port) != s.Port || len(cfg.Fallbacks) != 0 {
			t.Fatal("isolated cluster guard: DSNs must target only the owned private socket/database and expected roles")
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	conn, err := pgx.Connect(ctx, s.AdminURL)
	if err != nil {
		t.Fatal("isolated cluster guard: cannot connect to the private disposable database")
	}
	defer conn.Close(context.Background())
	var database, role, dataDir string
	if err := conn.QueryRow(ctx, "SELECT current_database(), current_user, current_setting('data_directory')").Scan(&database, &role, &dataDir); err != nil || database != s.Database || role != s.AdminRole || dataDir != s.DataDir {
		t.Fatal("isolated cluster guard: connected database/role/data directory does not match descriptor")
	}
	pidData, err := os.ReadFile(filepath.Join(s.DataDir, "postmaster.pid"))
	if err != nil || strings.Split(string(pidData), "\n")[0] != strconv.Itoa(s.ServerPID) || s.ServerPID <= 1 {
		t.Fatal("isolated cluster guard: private postmaster PID does not match descriptor")
	}
	return s
}

func pgControl(t *testing.T, s isolatedState, action string) {
	t.Helper()
	args := []string{action, "-D", s.DataDir, "-w", "-t", "20"}
	if action == "stop" {
		args = append(args, "-m", "fast")
	} else {
		args = append(args, "-l", filepath.Join(s.RootDir, "blackbox-postgres.log"))
	}
	ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
	defer cancel()
	if err := exec.CommandContext(ctx, s.PGCtl, args...).Run(); err != nil {
		t.Fatalf("private disposable PostgreSQL %s failed; inspect its private log", action)
	}
}

type serverProcess struct {
	cmd  *exec.Cmd
	done chan struct{}
	err  error
}

func startServer(t *testing.T, executable, dsn, address, logPath string) *serverProcess {
	t.Helper()
	logFile, err := os.OpenFile(logPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
	if err != nil {
		t.Fatal("cannot open private synthetic-server log")
	}
	cmd := exec.Command(executable, "-env-file", "")
	// Deliberately do not inherit DATABASE_URL, PG*, or any deployment env file.
	cmd.Env = []string{"PATH=" + os.Getenv("PATH"), "TZ=UTC", "DATABASE_URL=" + dsn,
		"VOID_HTTP_ADDR=" + address, "VOID_MESSAGING_ENABLED=true", "VOID_API_AUDIENCE=" + testAudience,
		"VOID_WORKER_ID=1001", "VOID_RATE_PER_MINUTE=10000", "VOID_RATE_BURST=1000"}
	cmd.Stdout, cmd.Stderr = logFile, logFile
	p := &serverProcess{cmd: cmd, done: make(chan struct{})}
	if err := cmd.Start(); err != nil {
		logFile.Close()
		t.Fatal("could not launch isolated server executable")
	}
	go func() {
		p.err = cmd.Wait()
		logFile.Close()
		close(p.done)
	}()
	t.Cleanup(func() { p.stop(t) })
	client := &http.Client{Timeout: time.Second}
	defer client.CloseIdleConnections()
	until := time.Now().Add(10 * time.Second)
	for time.Now().Before(until) {
		select {
		case <-p.done:
			t.Fatal("isolated server exited before readiness; inspect private synthetic-server log")
		default:
		}
		resp, err := client.Get("http://" + address + "/readyz")
		if err == nil {
			resp.Body.Close()
			if resp.StatusCode == http.StatusOK {
				return p
			}
		}
		time.Sleep(40 * time.Millisecond)
	}
	t.Fatal("isolated server did not become ready")
	return nil
}

func (p *serverProcess) stop(t *testing.T) {
	t.Helper()
	select {
	case <-p.done:
		return
	default:
	}
	_ = p.cmd.Process.Signal(syscall.SIGTERM)
	select {
	case <-p.done:
		if p.err != nil {
			t.Error("isolated server did not shut down cleanly")
		}
	case <-time.After(12 * time.Second):
		_ = p.cmd.Process.Kill()
		<-p.done
		t.Error("isolated server exceeded its shutdown deadline")
	}
}

type apiClient struct {
	t     *testing.T
	base  string
	token string
	http  *http.Client
}

func (a apiClient) call(method, path string, input any, status int, output any) {
	a.t.Helper()
	var body io.Reader
	if input != nil {
		encoded, err := json.Marshal(input)
		if err != nil {
			a.t.Fatal("cannot encode synthetic request")
		}
		body = bytes.NewReader(encoded)
	}
	req, err := http.NewRequest(method, a.base+apiPrefix+path, body)
	if err != nil {
		a.t.Fatal("cannot construct synthetic request")
	}
	if input != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if a.token != "" {
		req.Header.Set("Authorization", "Bearer "+a.token)
	}
	resp, err := a.http.Do(req)
	if err != nil {
		a.t.Fatalf("synthetic %s %s transport failed", method, path)
	}
	defer resp.Body.Close()
	if resp.StatusCode != status {
		var envelope struct {
			Error struct {
				Code string `json:"code"`
			} `json:"error"`
		}
		_ = json.NewDecoder(io.LimitReader(resp.Body, 4096)).Decode(&envelope)
		a.t.Fatalf("synthetic %s %s: status %d, expected %d (code %s)", method, path, resp.StatusCode, status, envelope.Error.Code)
	}
	if output != nil && json.NewDecoder(io.LimitReader(resp.Body, 8<<20)).Decode(output) != nil {
		a.t.Fatalf("synthetic %s %s returned invalid JSON", method, path)
	}
}

func uuid(t *testing.T) string {
	t.Helper()
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		t.Fatal("cannot generate synthetic UUID")
	}
	b[6], b[8] = b[6]&15|64, b[8]&63|128
	s := hex.EncodeToString(b[:])
	return s[:8] + "-" + s[8:12] + "-" + s[12:16] + "-" + s[16:20] + "-" + s[20:]
}

type identity struct {
	apiClient
	id, username, publicKey string
}

func registerAndAuthenticate(t *testing.T, base, name string) identity {
	t.Helper()
	public, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal("cannot generate synthetic Ed25519 identity")
	}
	client := &http.Client{Timeout: 12 * time.Second}
	t.Cleanup(client.CloseIdleConnections)
	a := apiClient{t: t, base: base, http: client}
	pub := base64.StdEncoding.EncodeToString(public)
	var challenge account.Challenge
	a.call("POST", "/challenges", map[string]string{"username": name, "auth_public_key": pub}, 201, &challenge)
	message, err := base64.StdEncoding.Strict().DecodeString(challenge.Message)
	if err != nil || !bytes.HasPrefix(message, []byte("VOID-NOTHINGNESS:register:v1\n")) {
		t.Fatal("registration challenge has invalid signing bytes")
	}
	var registered account.Account
	a.call("POST", "/accounts", map[string]string{"challenge_id": challenge.ChallengeID, "signature": base64.StdEncoding.EncodeToString(ed25519.Sign(private, message))}, 201, &registered)
	var authChallenge session.Challenge
	a.call("POST", "/auth/challenges", map[string]string{"void_id": registered.VoidID}, 201, &authChallenge)
	message, err = base64.StdEncoding.Strict().DecodeString(authChallenge.Message)
	lines := strings.Split(string(message), "\n")
	if err != nil || len(lines) != 8 || lines[0] != "VOID-NOTHINGNESS:auth:v1" || lines[1] != authChallenge.ChallengeID || lines[3] != registered.VoidID || lines[4] != pub || lines[5] != base64.StdEncoding.EncodeToString([]byte(testAudience)) || lines[7] != "" {
		t.Fatal("authentication challenge does not bind the registered identity and expected HTTPS audience")
	}
	var token session.Token
	a.call("POST", "/auth/sessions", map[string]string{"challenge_id": authChallenge.ChallengeID, "signature": base64.StdEncoding.EncodeToString(ed25519.Sign(private, message))}, 201, &token)
	if token.TokenType != "Bearer" || len(token.AccessToken) != 43 || !token.ExpiresAt.After(time.Now()) || token.ExpiresAt.After(time.Now().Add(31*time.Minute)) {
		t.Fatal("session does not provide a valid short-lived bearer")
	}
	a.token = token.AccessToken
	return identity{a, registered.VoidID, name, pub}
}

func (a identity) lookup(peer identity) {
	a.t.Helper()
	var person messaging.Person
	a.call("GET", "/people/"+peer.id, nil, 200, &person)
	if person.VoidID != peer.id || person.Username != peer.username || person.AuthPublicKey != peer.publicKey {
		a.t.Fatal("lookup did not return the exact HTTP-registered identity")
	}
}

func invitation(t *testing.T, from, to identity) messaging.Request {
	t.Helper()
	data := base64.StdEncoding.EncodeToString([]byte("synthetic opaque public KeyPackage:" + uuid(t)))
	var published struct {
		Packages []messaging.PublishedPackage `json:"packages"`
	}
	to.call("POST", "/key-packages", map[string]any{"packages": []string{data}}, 201, &published)
	if len(published.Packages) != 1 || published.Packages[0].State != "available" {
		t.Fatal("public KeyPackage publication failed")
	}
	claimInput := map[string]string{"client_claim_id": uuid(t)}
	var claim, retry messaging.Claim
	from.call("POST", "/people/"+to.id+"/key-packages/claim", claimInput, 200, &claim)
	from.call("POST", "/people/"+to.id+"/key-packages/claim", claimInput, 200, &retry)
	if claim != retry || claim.KeyPackageID != published.Packages[0].KeyPackageID || claim.KeyPackageBase64 != data || claim.Recipient.AuthPublicKey != to.publicKey {
		t.Fatal("claim or duplicate-safe claim retry changed the public package/key binding")
	}
	input := messaging.CreateRequestInput{ClientRequestID: uuid(t), ToVoidID: to.id, KeyPackageID: claim.KeyPackageID,
		GroupIDBase64: base64.StdEncoding.EncodeToString([]byte(uuid(t))), WelcomeBase64: base64.StdEncoding.EncodeToString([]byte("synthetic opaque Welcome"))}
	var result messaging.Request
	from.call("POST", "/requests", input, 201, &result)
	if result.Status != "pending" || result.Sender.VoidID != from.id || result.Recipient.VoidID != to.id || result.WelcomeBase64 != input.WelcomeBase64 {
		t.Fatal("invitation did not preserve account binding and opaque Welcome")
	}
	var list messaging.RequestPage
	to.call("GET", "/requests?limit=100", nil, 200, &list)
	found := false
	for _, row := range list.Requests {
		found = found || row.RequestID == result.RequestID && row.Status == "pending"
	}
	if !found {
		t.Fatal("recipient did not receive its pending invitation")
	}
	return result
}

func send(t *testing.T, from identity, conversation, kind string) (messaging.SendInput, messaging.Message) {
	t.Helper()
	input := messaging.SendInput{ClientMessageID: uuid(t), Kind: kind, CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("synthetic opaque wire bytes:" + uuid(t)))}
	var result messaging.Message
	from.call("POST", "/conversations/"+conversation+"/messages", input, 201, &result)
	if result.CiphertextBase64 != input.CiphertextBase64 || result.Kind != kind || result.ClientMessageID != input.ClientMessageID || result.SenderVoidID != from.id {
		t.Fatal("message response changed synthetic opaque bytes or sender binding")
	}
	return input, result
}

func pending(t *testing.T, actor identity) []messaging.Event {
	t.Helper()
	var all []messaging.Event
	after, last := "0", int64(0)
	seen := map[string]bool{}
	for page := 0; page < 100; page++ {
		var result messaging.SyncPage
		actor.call("GET", "/sync?after="+after+"&limit=2", nil, 200, &result)
		for _, event := range result.Events {
			cursor, err := strconv.ParseInt(event.Cursor, 10, 64)
			if err != nil || cursor <= last || seen[event.EventID] {
				t.Fatal("sync lost strict cursor ordering or repeated an event across pages")
			}
			last = cursor
			seen[event.EventID] = true
			all = append(all, event)
		}
		if !result.HasMore {
			return all
		}
		if result.NextCursor == after {
			t.Fatal("sync cursor did not advance")
		}
		after = result.NextCursor
	}
	t.Fatal("sync pagination did not terminate")
	return nil
}

func ack(t *testing.T, actor identity, events []messaging.Event, rejectedMessage string) []messaging.Acknowledgment {
	t.Helper()
	acks := make([]messaging.Acknowledgment, 0, len(events))
	for _, event := range events {
		outcome := "applied"
		if event.Type == "message.created" {
			var message messaging.Message
			if json.Unmarshal(event.Payload, &message) != nil {
				t.Fatal("invalid message event")
			}
			if message.MessageID == rejectedMessage {
				outcome = "rejected"
			}
		}
		acks = append(acks, messaging.Acknowledgment{EventID: event.EventID, Outcome: outcome})
	}
	if len(acks) != 0 {
		actor.call("POST", "/sync/ack", map[string]any{"events": acks}, 204, nil)
	}
	return acks
}

func requireMessages(t *testing.T, events []messaging.Event, expected ...messaging.Message) {
	t.Helper()
	var actual []messaging.Message
	for _, event := range events {
		if event.Type == "message.created" {
			var message messaging.Message
			if json.Unmarshal(event.Payload, &message) != nil {
				t.Fatal("invalid message event payload")
			}
			actual = append(actual, message)
		}
	}
	if !reflect.DeepEqual(actual, expected) {
		t.Fatal("message events changed bytes, order, sequence or exactly-once enqueue identity")
	}
}

func requireReceipts(t *testing.T, events []messaging.Event, expected map[string]string) {
	t.Helper()
	if len(events) != len(expected) {
		t.Fatal("unexpected receipt count or duplicate receipt")
	}
	seen := map[string]bool{}
	for _, event := range events {
		var receipt messaging.Receipt
		if json.Unmarshal(event.Payload, &receipt) != nil || seen[receipt.MessageID] || expected[receipt.MessageID] != event.Type ||
			(event.Type == "message.delivered" && receipt.Outcome != "applied") || (event.Type == "message.rejected" && receipt.Outcome != "rejected") {
			t.Fatal("receipt identity, event type, outcome or uniqueness is incorrect")
		}
		seen[receipt.MessageID] = true
	}
}

// discardSuccessfulResponse simulates losing the response after the server has
// committed. The application observes a transport error; the test retains the
// discarded response solely to verify the retry returns the original server ID.
type discardSuccessfulResponse struct {
	transport http.RoundTripper
	discarded messaging.Message
}

var errResponseLost = errors.New("synthetic response loss after committed send")

func (d *discardSuccessfulResponse) RoundTrip(req *http.Request) (*http.Response, error) {
	resp, err := d.transport.RoundTrip(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 201 || json.NewDecoder(resp.Body).Decode(&d.discarded) != nil {
		return nil, errors.New("synthetic loss transport did not receive a committed message")
	}
	return nil, errResponseLost
}

func TestMessagingHTTPPostgresProcessRestart(t *testing.T) {
	s := loadIsolatedState(t)
	repo, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal("cannot resolve source checkout")
	}
	buildDir := t.TempDir()
	executable := filepath.Join(buildDir, "void-server")
	build := exec.Command("go", "build", "-o", executable, "./cmd/void-server")
	build.Dir = repo
	if output, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build isolated server executable: %s", output)
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal("cannot allocate loopback test port")
	}
	address := listener.Addr().String()
	listener.Close()
	server := startServer(t, executable, s.AppURL, address, filepath.Join(buildDir, "server.log"))
	base := "http://" + address
	alice := registerAndAuthenticate(t, base, "synthetic-blackbox-alice")
	bob := registerAndAuthenticate(t, base, "synthetic-blackbox-bob")
	alice.lookup(bob)
	bob.lookup(alice)
	t.Log("two fresh identities registered and authenticated through real HTTP; lookup preserves registered public keys")

	declined := invitation(t, alice, bob)
	var decision messaging.Request
	bob.call("POST", "/requests/"+declined.RequestID+"/decline", struct{}{}, 200, &decision)
	if decision.Status != "declined" {
		t.Fatal("recipient decline did not persist")
	}
	accepted := invitation(t, alice, bob)
	bob.call("POST", "/requests/"+accepted.RequestID+"/accept", struct{}{}, 200, &decision)
	if decision.Status != "accepted" {
		t.Fatal("recipient acceptance did not persist")
	}
	for _, actor := range []identity{alice, bob} {
		var list messaging.ConversationPage
		actor.call("GET", "/conversations", nil, 200, &list)
		if len(list.Conversations) != 1 || list.Conversations[0].ConversationID != accepted.ConversationID {
			t.Fatal("accepted conversation is not visible to both participants")
		}
	}

	firstInput := messaging.SendInput{ClientMessageID: uuid(t), Kind: "application", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("synthetic opaque application with a lost HTTP response"))}
	encoded, _ := json.Marshal(firstInput)
	req, _ := http.NewRequest("POST", base+apiPrefix+"/conversations/"+accepted.ConversationID+"/messages", bytes.NewReader(encoded))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+alice.token)
	transport := http.DefaultTransport.(*http.Transport).Clone()
	defer transport.CloseIdleConnections()
	loss := &discardSuccessfulResponse{transport: transport}
	lostClient := &http.Client{Transport: loss, Timeout: 12 * time.Second}
	if _, err := lostClient.Do(req); !errors.Is(err, errResponseLost) {
		t.Fatal("send did not produce the deliberately lost successful response")
	}
	var first messaging.Message
	alice.call("POST", "/conversations/"+accepted.ConversationID+"/messages", firstInput, 201, &first)
	if !reflect.DeepEqual(first, loss.discarded) {
		t.Fatal("retry after lost response did not return the original message ID/sequence")
	}
	changed := firstInput
	changed.CiphertextBase64 = base64.StdEncoding.EncodeToString([]byte("changed opaque ciphertext"))
	alice.call("POST", "/conversations/"+accepted.ConversationID+"/messages", changed, 409, nil)
	_, second := send(t, bob, accepted.ConversationID, "application")
	_, third := send(t, alice, accepted.ConversationID, "commit")
	if first.Sequence != "1" || second.Sequence != "2" || third.Sequence != "3" {
		t.Fatal("application/commit messages do not share strict conversation sequence ordering")
	}
	bobEvents, aliceEvents := pending(t, bob), pending(t, alice)
	requireMessages(t, bobEvents, first, third)
	requireMessages(t, aliceEvents, second)
	ack(t, bob, bobEvents, third.MessageID)
	ack(t, alice, aliceEvents, "")
	aliceReceipts, bobReceipts := pending(t, alice), pending(t, bob)
	requireReceipts(t, aliceReceipts, map[string]string{first.MessageID: "message.delivered", third.MessageID: "message.rejected"})
	requireReceipts(t, bobReceipts, map[string]string{second.MessageID: "message.delivered"})
	ack(t, alice, aliceReceipts, "")
	ack(t, bob, bobReceipts, "")
	if len(pending(t, alice)) != 0 || len(pending(t, bob)) != 0 {
		t.Fatal("acknowledged events were redelivered before restart")
	}
	t.Log("public packages, decline/accept, both message directions, commit ordering, lost response retry and applied/rejected receipts verified over HTTP")

	// Bob performs no HTTP operations between enqueue and the full restart.
	offlineInput, offline := send(t, alice, accepted.ConversationID, "application")
	if offline.Sequence != "4" {
		t.Fatal("offline enqueue did not preserve sequence order")
	}
	server.stop(t)
	alice.http.CloseIdleConnections()
	bob.http.CloseIdleConnections()
	pgControl(t, s, "stop")
	// If a later restart fails, cleanup makes a best effort to leave this owned
	// disposable cluster running for its provisioning runner to inspect/remove.
	pgStopped := true
	defer func() {
		if pgStopped {
			ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
			defer cancel()
			_ = exec.CommandContext(ctx, s.PGCtl, "start", "-D", s.DataDir, "-l", filepath.Join(s.RootDir, "blackbox-postgres.log"), "-w", "-t", "20").Run()
		}
	}()
	pgControl(t, s, "start")
	pgStopped = false
	server = startServer(t, executable, s.AppURL, address, filepath.Join(buildDir, "server.log"))
	_ = server
	alice.lookup(bob)
	bob.lookup(alice)
	var offlineRetry messaging.Message
	alice.call("POST", "/conversations/"+accepted.ConversationID+"/messages", offlineInput, 201, &offlineRetry)
	if !reflect.DeepEqual(offlineRetry, offline) {
		t.Fatal("persisted outbound retry changed identity/sequence after server and PostgreSQL restart")
	}
	offlineEvents := pending(t, bob)
	if len(offlineEvents) != 1 {
		t.Fatal("offline sync did not return exactly one unacknowledged message after full restart")
	}
	requireMessages(t, offlineEvents, offline)
	if repeated := pending(t, bob); !reflect.DeepEqual(repeated, offlineEvents) {
		t.Fatal("unacknowledged offline event was not replayed with stable identity")
	}
	acks := ack(t, bob, offlineEvents, "")
	bob.call("POST", "/sync/ack", map[string]any{"events": acks}, 204, nil)
	conflictingAck := []messaging.Acknowledgment{{EventID: acks[0].EventID, Outcome: "rejected"}}
	bob.call("POST", "/sync/ack", map[string]any{"events": conflictingAck}, 409, nil)
	if len(pending(t, bob)) != 0 {
		t.Fatal("offline event was redelivered after acknowledgment")
	}
	offlineReceipts := pending(t, alice)
	requireReceipts(t, offlineReceipts, map[string]string{offline.MessageID: "message.delivered"})
	ack(t, alice, offlineReceipts, "")
	if len(pending(t, alice)) != 0 {
		t.Fatal("duplicate acknowledgment created a duplicate receipt")
	}
	t.Log("actual server and PostgreSQL processes restarted; existing bearers, offline bytes, stable retry IDs, ACKs and deduplication survived")

	bob.call("DELETE", "/auth/session", nil, 204, nil)
	bob.call("GET", "/people/"+alice.id, nil, 401, nil)
	alice.call("PUT", "/blocks/"+bob.id, struct{}{}, 204, nil)
	alice.call("GET", "/people/"+bob.id, nil, 404, nil)
	blockedInput := messaging.SendInput{ClientMessageID: uuid(t), Kind: "application", CiphertextBase64: "AQIDBA=="}
	alice.call("POST", "/conversations/"+accepted.ConversationID+"/messages", blockedInput, 404, nil)
	t.Log("logout invalidates the persisted bearer; block prevents lookup and new messages")
}
