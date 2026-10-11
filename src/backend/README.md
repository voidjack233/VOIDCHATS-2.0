# Development backend connectivity

In a **debug** build, open **Profile → Settings → Development → Backend connection**. Enter the development server's origin, then select **Check connection**. The selection stays only in the current JavaScript session; reload resets it. There is no bundled default or mobile `.env` file.

- On a physical Android phone, enter `http://<host-lan-ip>:8080` after starting the API on an explicit LAN listener. Replace the placeholder with the computer's actual IPv4 address, use the same trusted network, and keep PostgreSQL bound to the computer's loopback interface. `localhost` on the phone refers to the phone itself.
- Enter `https://<configured-hostname>` for an authorized named tunnel, or the actual HTTPS URL printed by a permitted health-only development tunnel. Do not expose the unauthenticated account API for unrestricted public writes.
- Enter only an origin: an IPv4 address or DNS hostname, optionally a port. Paths, query strings, fragments, embedded credentials, and IPv6 literals are rejected. No IP or tunnel hostname is hardcoded into the app.

The diagnostic sends only `GET /health`, omits credentials, verifies HTTP 200 plus JSON `{"status":"ok"}`, and aborts after eight seconds. It creates no account and does not access the native OpenMLS implementation or device identity. It reports HTTP failures, unexpected tunnel login pages, connection failures, and timeouts without displaying response bodies or logging keys.

For a connection failure, first verify the health URL from the phone's browser. Check the API's LAN bind address, the phone's network, and a narrow firewall exception on the trusted private network. For an Access-protected tunnel, use the project's supported interactive authentication workflow; this diagnostic does not implement tunnel authentication. Cloudflare service tokens, database credentials, and account private keys must never be entered into this selector or bundled into the app.

Release builds cannot select a development URL or run the diagnostic, and configuration validation requires HTTPS. The Settings screen loads the diagnostic only under `__DEV__`. Android's main manifest denies cleartext; HTTP permission and its network-security resource exist only in `android/app/src/debug/`. iOS transport security remains unchanged. These settings follow the [Android network security configuration](https://developer.android.com/privacy-and-security/security-config) and use React Native's [Fetch networking](https://reactnative.dev/docs/network).

Run from the repository root:

```sh
npm run typecheck
npm run lint
node --test src/backend/backend.test.mjs
```

The tests use Node's built-in test runner and the existing TypeScript compiler. They check URL restrictions, release guards, the exact health-only request, bad responses, and request cancellation at the deadline. Physical-device connectivity still needs manual verification with a debug build on the target phone.
