import { api } from "@/lib/session";
import Link from "next/link";
import { notFound } from "next/navigation";

export default async function RepoLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ owner: string; name: string }>;
}) {
  const { owner, name } = await params;
  const result = await api(`/v1/repos/${owner}/${name}`);
  if (result.status === 404) notFound();
  const repo = result.json as { role?: string; default_ref?: string };
  const base = `/r/${owner}/${name}`;
  return (
    <>
      <p className="muted">
        <Link href="/">Repositories</Link> / {owner}/{name} · role {repo.role} · {repo.default_ref}
      </p>
      <nav className="row" style={{ marginBottom: "1rem" }}>
        <Link href={base}>Overview</Link>
        <Link href={`${base}/tree`}>Tree</Link>
        <Link href={`${base}/history`}>History</Link>
        <Link href={`${base}/refs`}>Refs</Link>
        <Link href={`${base}/settings`}>Settings</Link>
      </nav>
      {children}
    </>
  );
}
