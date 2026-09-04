import "./style.css";
import { aggregate } from "./state";
import { fetchSessions } from "./poller";
import { render } from "./renderer";
import { restorePosition, rememberPosition } from "./window-pos";
import "./menu";

const POLL_INTERVAL_MS = 1000;

async function tick(): Promise<void> {
  try {
    const sessions = await fetchSessions();
    const now = Math.floor(Date.now() / 1000);
    render(aggregate(sessions, now));
  } catch (e) {
    console.error("poll failed:", e);
    render("rest");
  }
  void rememberPosition();
}

async function boot(): Promise<void> {
  await restorePosition();
  void tick();
  setInterval(tick, POLL_INTERVAL_MS);
}

void boot();
