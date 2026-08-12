/**
 * The update prompt.
 *
 * One piece of interface for two quite different mechanisms underneath — on Windows the
 * Tauri updater verifies a signature and swaps the binary; on Android the app downloads
 * an APK, checks its hash, and hands it to the system installer. See
 * `src-tauri/src/updater.rs` for why those cannot be the same code.
 *
 * The check is deliberately quiet: a banner, dismissible, never a modal. Interrupting
 * someone mid-drag to tell them about a patch release is a way to make people turn
 * updates off.
 */

import { createSignal, onMount, Show } from "solid-js";
import * as ipc from "./ipc";
import type { UpdateInfo } from "./types";

export function Updates() {
  const [info, setInfo] = createSignal<UpdateInfo | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [dismissed, setDismissed] = createSignal(false);
  const [failed, setFailed] = createSignal<string | null>(null);

  onMount(() => {
    // Delayed so a cold start is not competing with a network request for the main
    // thread while the person is waiting to see their document.
    window.setTimeout(() => {
      ipc.updates
        .check()
        .then((result) => {
          if (result.available) setInfo(result);
        })
        .catch(() => {
          // Being offline is not an error worth showing anyone.
        });
    }, 4000);
  });

  async function install() {
    setBusy(true);
    setFailed(null);
    try {
      await ipc.updates.install();
    } catch (e) {
      // The Android hand-off can be declined, and some devices refuse the install
      // intent outright. Falling back to the release page always works.
      setFailed(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Show when={dismissed() ? null : info()}>
      {(update) => (
        <div class="update" role="status">
          <div class="update__text">
            <strong>Version {update().latestVersion} is available.</strong>
            <Show when={update().notes}>
              {(notes) => <span class="update__notes">{notes()}</span>}
            </Show>
            <Show when={failed()}>
              {(reason) => (
                <span class="update__error">
                  {reason()} — you can download it manually instead.
                </span>
              )}
            </Show>
          </div>

          <div class="update__actions">
            <Show when={failed()}>
              <button onClick={() => void ipc.updates.openReleasePage()}>Open release</button>
            </Show>
            <button class="update__go" disabled={busy()} onClick={() => void install()}>
              {busy() ? "Downloading…" : "Update"}
            </button>
            <button onClick={() => setDismissed(true)} aria-label="Dismiss">
              Later
            </button>
          </div>
        </div>
      )}
    </Show>
  );
}
