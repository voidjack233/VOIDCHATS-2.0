# Project workflow

- This checkout owns the React Native/TypeScript frontend and Android native integration. Backend implementation belongs to the user's separate backend agent. When backend work is needed, provide a concrete proposal with protocol details, storage implications and frontend dependencies. Do not recreate a backend checkout, edit backend source, or apply server SQL unless the user explicitly changes this workflow.
- Tests, builds, APK generation, deployments and database changes remain paused until explicitly authorized. Source inspection and authorized implementation may continue; report unverified behavior accurately.
- Preserve the entire native/openmls/ directory without deleting, moving, rewriting or refactoring it. OpenMLS is dormant for future groups and excluded from the direct-messaging application. Use official libsignal for 1:1 messaging; preserve existing account identities and local data.
