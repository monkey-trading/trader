/** The note on an account the daily loss limit has stopped, with when trading resumes. */
import type { DayStop } from "@/api/types";
import { formatDayStop } from "@/lib/format";

/** The note, in the browser's local time and in UTC. */
export function DayStopNote({ dayStop }: { readonly dayStop: DayStop }) {
  // `getTimezoneOffset` is minutes behind UTC, so the offset east of UTC is its negative.
  const offsetMinutes = -new Date(dayStop.untilMs).getTimezoneOffset();
  return <span className="block text-warn">{formatDayStop(dayStop, offsetMinutes)}</span>;
}
