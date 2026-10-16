"use client";

import { call } from "@/lib/call";
import { useEffect, useState } from "react";

type Row = { id: string; created_at: number; last_seen_at: number; current: boolean };

export default function SessionsPage() {
  const [rows, setRows] = useState<Row[]>([]);

  async function load() {
    const result = await call("/v1/auth/sessions", "GET");
    setRows(((result.json as { sessions?: Row[] }).sessions) ?? []);
  }

  useEffect(() => {
    let cancelled = false;
    call("/v1/auth/sessions", "GET").then((result) => {
      if (!cancelled) setRows(((result.json as { sessions?: Row[] }).sessions) ?? []);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <>
      <h1>Sessions</h1>
      <table>
        <tbody>
          {rows.map((row) => (
            <tr key={row.id}>
              <td className="mono">{row.id.slice(0, 12)}</td>
              <td>{row.current ? "this session" : "other"}</td>
              <td>
                {row.current ? null : (
                  <button
                    type="button"
                    onClick={async () => {
                      await call(`/v1/auth/sessions/${row.id}`, "DELETE");
                      await load();
                    }}
                  >
                    Revoke
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}
