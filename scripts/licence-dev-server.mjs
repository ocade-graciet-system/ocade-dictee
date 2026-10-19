#!/usr/bin/env node
// Serveur de licences **de test**, en mémoire, pour développer et recetter
// l'onglet Licence d'OCADE Dictée sans PostgreSQL.
//
// La référence reste le vrai serveur, ocade-graciet-system/ocade-dictee-licences
// (src/services/licensing.ts, src/services/errors.ts) : ce script en reproduit
// l'API publique (POST /api/v1/activate | refresh | release, GET /mode | health),
// la forme des erreurs (`{ error: { code, message, ...extra }, server_time }`)
// et le jeton signé. Il n'en reproduit ni la base, ni la supervision, ni les
// limites de débit. Quand un PostgreSQL local est disponible, préférez le vrai.
//
// Il ajoute des routes de pilotage /_dev/* pour simuler ce que fera la
// supervision (créer une clé, révoquer, expirer, changer le mode…).
//
// Usage :
//   node scripts/licence-dev-server.mjs            (port 8787)
// puis lancer l'application avec les variables affichées au démarrage
// (OCADE_LICENCE_URL et OCADE_LICENCE_PUBKEYS), par exemple :
//   $env:OCADE_LICENCE_URL = "http://127.0.0.1:8787/api/v1"
//   $env:OCADE_LICENCE_PUBKEYS = "k1:<clé publique affichée>"
//   .\dev-windows.ps1
//
// Pilotage (exemples PowerShell) :
//   irm -Method Post http://127.0.0.1:8787/_dev/licenses -Body '{"seats":2,"days":45}' -ContentType application/json
//   irm -Method Post http://127.0.0.1:8787/_dev/licenses/OCAD-…/revoke
//   irm -Method Post http://127.0.0.1:8787/_dev/licenses/OCAD-…/enable
//   irm -Method Post http://127.0.0.1:8787/_dev/licenses/OCAD-…/archive
//   irm -Method Post http://127.0.0.1:8787/_dev/licenses/OCAD-…/expires -Body '{"days":5}' -ContentType application/json
//   irm -Method Post http://127.0.0.1:8787/_dev/licenses/OCAD-…/delete
//   irm -Method Post http://127.0.0.1:8787/_dev/mode -Body '{"mode":"strict"}' -ContentType application/json
//   irm http://127.0.0.1:8787/_dev/state
//
// La paire de clés de test est conservée dans le dossier temporaire du
// système, pour que la clé publique reste la même d'un lancement à l'autre.
// Ce serveur n'a AUCUNE sécurité : ne jamais l'exposer hors de la machine.

import http from "node:http";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const PORT = Number(process.env.PORT ?? 8787);
const KID = "k1";
const TOKEN_TTL = 14 * 86400;
const ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

// --- Paire de clés de test, stable ------------------------------------------
const keyFile = path.join(os.tmpdir(), "ocade-licence-dev-key.pem");
let privateKey;
if (fs.existsSync(keyFile)) {
  privateKey = crypto.createPrivateKey(fs.readFileSync(keyFile));
} else {
  privateKey = crypto.generateKeyPairSync("ed25519").privateKey;
  fs.writeFileSync(
    keyFile,
    privateKey.export({ type: "pkcs8", format: "pem" }),
  );
}
const publicRaw = crypto
  .createPublicKey(privateKey)
  .export({ type: "spki", format: "der" })
  .subarray(-32);
// Base64 standard, comme `npm run gen:key` du vrai serveur.
const publicB64 = publicRaw.toString("base64");

// --- Clés : Luhn mod 32 (identique à src-tauri/src/licence/key.rs) ----------
function checkChar(body) {
  let factor = 2;
  let sum = 0;
  for (let i = body.length - 1; i >= 0; i--) {
    let addend = factor * ALPHABET.indexOf(body[i]);
    factor = factor === 2 ? 1 : 2;
    addend = Math.floor(addend / 32) + (addend % 32);
    sum += addend;
  }
  return ALPHABET[(32 - (sum % 32)) % 32];
}
function newKey() {
  let body = "";
  for (let i = 0; i < 15; i++) body += ALPHABET[crypto.randomInt(32)];
  const full = body + checkChar(body);
  return `OCAD-${full.slice(0, 4)}-${full.slice(4, 8)}-${full.slice(8, 12)}-${full.slice(12)}`;
}
function normalize(input) {
  const compact = String(input ?? "")
    .replace(/[\s-]/g, "")
    .toUpperCase();
  const body = compact.startsWith("OCAD") ? compact.slice(4) : compact;
  if (body.length !== 16 || [...body].some((c) => !ALPHABET.includes(c)))
    return null;
  if (checkChar(body.slice(0, 15)) !== body[15]) return null;
  return `OCAD-${body.slice(0, 4)}-${body.slice(4, 8)}-${body.slice(8, 12)}-${body.slice(12)}`;
}

// --- État en mémoire ----------------------------------------------------------
const licenses = new Map(); // key -> licence
let mode = "soft";
const now = () => Math.floor(Date.now() / 1000);

function state(lic) {
  if (lic.archived) return "archived";
  if (!lic.enabled) return "revoked";
  if (lic.expires_at !== null && lic.expires_at <= now()) return "expired";
  return "active";
}

function sign(payload) {
  const enc = (o) => Buffer.from(JSON.stringify(o)).toString("base64url");
  const head = enc({ alg: "EdDSA", typ: "OCADE-LIC", kid: KID });
  const body = enc(payload);
  const sig = crypto.sign(null, Buffer.from(`${head}.${body}`), privateKey);
  return `${head}.${body}.${sig.toString("base64url")}`;
}

/** Réponse de succès d'activate et refresh, comme `LicensingService.issue`. */
function tokenFor(lic, act) {
  const iat = now();
  const exp =
    lic.expires_at === null
      ? iat + TOKEN_TTL
      : Math.min(iat + TOKEN_TTL, lic.expires_at);
  const used = lic.activations.filter((a) => a.state === "active").length;
  const seats = { max: lic.seats_max, used };
  const keySuffix = lic.key.slice(-4);
  const token = sign({
    v: 1,
    lid: lic.id,
    aid: act.id,
    fp: act.fingerprint,
    kfx: keySuffix,
    iat,
    exp,
    lexp: lic.expires_at,
    seats,
  });
  return {
    token,
    license: { key_suffix: keySuffix, expires_at: lic.expires_at, seats },
    server_time: iat,
  };
}

// --- HTTP -----------------------------------------------------------------------
function send(res, status, body) {
  res.writeHead(status, { "content-type": "application/json; charset=utf-8" });
  res.end(JSON.stringify(body));
}
function refuse(res, status, code, extra = {}) {
  console.log(`  -> refus ${code}`);
  // Comme le vrai serveur : les champs supplémentaires sont DANS `error`.
  send(res, status, {
    error: { code, message: code, ...extra },
    server_time: now(),
  });
}
async function readJson(req) {
  let raw = "";
  for await (const chunk of req) raw += chunk;
  try {
    return raw ? JSON.parse(raw) : {};
  } catch {
    return null;
  }
}

function findForClient(body, res) {
  const key = normalize(body?.key);
  if (!key) return void refuse(res, 400, "invalid_key_format");
  const lic = licenses.get(key);
  if (!lic) return void refuse(res, 404, "unknown_key");
  return lic;
}
function refuseIfInactive(lic, res) {
  const s = state(lic);
  if (s === "active") return false;
  const status = 403;
  refuse(res, status, s, s === "expired" ? { expired_at: lic.expires_at } : {});
  return true;
}
function touch(act, body) {
  Object.assign(act, {
    hostname: body.hostname,
    os: body.os,
    app_version: body.app_version,
    last_contact_at: now(),
  });
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const route = `${req.method} ${url.pathname}`;
  console.log(`${new Date().toISOString().slice(11, 19)} ${route}`);

  if (route === "GET /api/v1/health") return send(res, 200, { ok: true });
  if (route === "GET /api/v1/mode")
    return send(res, 200, { mode, server_time: now() });

  if (req.method === "POST" && url.pathname.startsWith("/api/v1/")) {
    const body = await readJson(req);
    if (!body) return refuse(res, 400, "invalid_request");
    const lic = findForClient(body, res);
    if (!lic) return;
    const active = lic.activations.find(
      (a) => a.state === "active" && a.fingerprint === body.fingerprint,
    );

    if (url.pathname === "/api/v1/release") {
      if (!active) return refuse(res, 403, "not_activated");
      const yearAgo = now() - 365 * 86400;
      const recent = lic.releases.filter((t) => t > yearAgo).length;
      if (lic.release_quota !== null && recent >= lic.release_quota)
        return refuse(res, 403, "release_quota_exceeded");
      active.state = "released";
      lic.releases.push(now());
      return send(res, 200, {
        released: true,
        remaining_quota:
          lic.release_quota === null ? null : lic.release_quota - recent - 1,
        server_time: now(),
      });
    }

    if (refuseIfInactive(lic, res)) return;

    if (url.pathname === "/api/v1/refresh") {
      if (!active) return refuse(res, 403, "not_activated");
      touch(active, body);
      return send(res, 200, tokenFor(lic, active));
    }

    if (url.pathname === "/api/v1/activate") {
      let act = active;
      if (!act) {
        const used = lic.activations.filter((a) => a.state === "active").length;
        if (used >= lic.seats_max)
          return refuse(res, 409, "seats_full", {
            seats: { max: lic.seats_max, used },
          });
        act = {
          id: crypto.randomUUID(),
          fingerprint: body.fingerprint,
          state: "active",
        };
        lic.activations.push(act);
      }
      touch(act, body);
      console.log(
        `  -> activée sur ${body.hostname} (${body.os?.name} ${body.os?.version})`,
      );
      return send(res, 200, tokenFor(lic, act));
    }
  }

  // --- Pilotage de test -------------------------------------------------------
  if (route === "GET /_dev/state")
    return send(res, 200, {
      mode,
      licenses: [...licenses.values()].map((l) => ({ ...l, state: state(l) })),
    });
  if (route === "POST /_dev/mode") {
    const body = await readJson(req);
    if (!["soft", "strict"].includes(body?.mode))
      return refuse(res, 400, "invalid_request");
    mode = body.mode;
    return send(res, 200, { mode });
  }
  if (route === "POST /_dev/licenses") {
    const body = (await readJson(req)) ?? {};
    const key = newKey();
    const lic = {
      id: crypto.randomUUID(),
      key,
      title: body.title ?? "Licence de test",
      enabled: true,
      archived: false,
      expires_at:
        body.days === null ? null : now() + (body.days ?? 365) * 86400,
      seats_max: body.seats ?? 1,
      release_quota: body.release_quota === undefined ? 3 : body.release_quota,
      activations: [],
      releases: [],
    };
    licenses.set(key, lic);
    console.log(`  -> nouvelle clé ${key}`);
    return send(res, 200, lic);
  }
  const m = url.pathname.match(/^\/_dev\/licenses\/([^/]+)\/(\w+)$/);
  if (req.method === "POST" && m) {
    const key = normalize(decodeURIComponent(m[1]));
    const lic = key && licenses.get(key);
    if (!lic) return refuse(res, 404, "unknown_key");
    const body = (await readJson(req)) ?? {};
    switch (m[2]) {
      case "revoke":
        lic.enabled = false;
        break;
      case "enable":
        lic.enabled = true;
        break;
      case "archive":
        lic.archived = true;
        break;
      case "restore":
        lic.archived = false;
        break;
      case "expires":
        lic.expires_at = body.days === null ? null : now() + body.days * 86400;
        break;
      case "release-all":
        lic.activations.forEach((a) => (a.state = "released"));
        break;
      case "delete":
        licenses.delete(key);
        return send(res, 200, { deleted: key });
      default:
        return refuse(res, 404, "invalid_request");
    }
    return send(res, 200, { ...lic, state: state(lic) });
  }

  send(res, 404, { error: { code: "not_found", message: "route inconnue" } });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`Serveur de licences de TEST sur http://127.0.0.1:${PORT}`);
  console.log(
    "Variables à définir avant de lancer l'application (PowerShell) :",
  );
  console.log(`  $env:OCADE_LICENCE_URL = "http://127.0.0.1:${PORT}/api/v1"`);
  console.log(`  $env:OCADE_LICENCE_PUBKEYS = "${KID}:${publicB64}"`);
});
