# Source-control ownership

This directory is the reviewed source of the Anticaptrad YouTube Google Apps Script web app associated with Linear DEN-399 and Script ID `17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ`.

The Rust server remains the authenticated control plane. Apps Script owns Google OAuth and Google API calls. Public HTTP deployment must use the `http-api` profile, while the owner dashboard uses an owner-only profile.

Deployment changes require `npm run check`, a feature branch, and pull-request review. Never commit generated API keys, OAuth tokens, Drive identifiers containing private data, or channel credentials.
