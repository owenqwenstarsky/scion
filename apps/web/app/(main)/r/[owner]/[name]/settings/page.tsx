import { api } from "@/lib/session";
import Link from "next/link";

export default async function SettingsPage({ params }: { params: Promise<{ owner: string; name: string }> }) {
  const { owner, name } = await params;
  const result = await api(`/v1/repos/${owner}/${name}`);
  const caps = (result.json as { capabilities?: Record<string, boolean> }).capabilities ?? {};
  return (
    <>
      <h1>Settings</h1>
      <ul>
        {caps.can_manage_members ? <li><Link href={`/r/${owner}/${name}/settings/members`}>Members</Link></li> : null}
        {caps.can_admin_repo ? <li><Link href={`/r/${owner}/${name}/settings/paths`}>Path rules</Link></li> : null}
        <li><Link href={`/r/${owner}/${name}/settings/audit`}>Audit</Link></li>
      </ul>
      <p className="muted">Role capabilities are enforced by the server. Hiding a link does not grant access.</p>
    </>
  );
}
