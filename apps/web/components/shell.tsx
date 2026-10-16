import Link from "next/link";
import type { SessionView } from "@/lib/session";

export function Shell({ session, children }: { session: SessionView; children: React.ReactNode }) {
  return (
    <>
      <header className="top">
        <strong><Link href="/">Scion</Link></strong>
        <nav>
          <Link href="/repos/new">New repository</Link>
          <Link href="/settings/tokens">Tokens</Link>
          <Link href="/settings/sessions">Sessions</Link>
          {session.site_admin ? <Link href="/admin/users">Users</Link> : null}
          {session.site_admin ? <Link href="/admin/audit">Audit</Link> : null}
          {session.site_admin ? <Link href="/admin/health">Health</Link> : null}
          <span className="muted">{session.username}</span>
          <form action="/api/logout" method="post">
            <button className="quiet" type="submit">Sign out</button>
          </form>
        </nav>
      </header>
      <main>{children}</main>
    </>
  );
}
