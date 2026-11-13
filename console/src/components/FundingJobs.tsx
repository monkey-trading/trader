/** The funding jobs: each action's state until it lands, and each transaction's. */
import { useEffect, useRef, useState } from "react";

import { ended, type FundingJob } from "@/api/funding-types";
import { shortAddress } from "@/lib/format";

import { CELL, DataTable } from "./DataTable";

const HEAD = ["Action", "Account", "State", "Result", "Transactions"] as const;
const STATES: Readonly<Record<FundingJob["state"], string>> = {
  waiting: "Waiting", running: "Running", landed: "Landed", failed: "Failed",
};

/** What a job that just ended says, for a screen reader. */
function announcement(job: FundingJob): string {
  return `${job.action} for ${shortAddress(job.account)} ${job.state === "landed" ? "landed" : "failed"}: ${job.detail}`;
}

/** The jobs, newest first, and a live region that says each one's end once. */
export function FundingJobs({ jobs }: { readonly jobs: readonly FundingJob[] }) {
  const before = useRef<ReadonlyMap<number, boolean> | null>(null);
  const [said, setSaid] = useState("");
  useEffect(() => {
    const seen = before.current;
    const justEnded = seen === null ? [] : jobs.filter((job) => ended(job) && seen.get(job.id) === false);
    before.current = new Map(jobs.map((job) => [job.id, ended(job)]));
    if (justEnded.length > 0) {
      setSaid(justEnded.map(announcement).join(". "));
    }
  }, [jobs]);
  return (
    <>
      <p aria-live="polite" className="sr-only">{said}</p>
      {jobs.length === 0 ? <p className="text-muted">No action has been started.</p> : <JobsTable jobs={jobs} />}
    </>
  );
}

function JobsTable({ jobs }: { readonly jobs: readonly FundingJob[] }) {
  return (
    <DataTable label="Funding actions" head={HEAD}>
      {[...jobs].reverse().map((job) => (
        <tr key={job.id} className="border-t border-line">
          <td className={CELL}>{job.action}</td>
          <th scope="row" className={`${CELL} font-medium`}><span title={job.account}>{shortAddress(job.account)}</span></th>
          <td className={`${CELL} ${job.state === "failed" ? "text-loss" : ""}`}>{STATES[job.state]}</td>
          <td className="px-2 py-1">{job.detail}</td>
          <td className="px-2 py-1 text-xs">
            {job.txs.map((tx) => (
              <div key={tx.hash}>
                {`${tx.what}: ${tx.state} `}
                <span className="font-mono break-all">{tx.hash}</span>
              </div>
            ))}
          </td>
        </tr>
      ))}
    </DataTable>
  );
}
