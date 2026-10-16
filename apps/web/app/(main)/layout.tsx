import { Shell } from "@/components/shell";
import { requireSession } from "@/lib/session";
import { headers } from "next/headers";
import { redirect } from "next/navigation";

export default async function MainLayout({ children }: { children: React.ReactNode }) {
  const session = await requireSession();
  const path = (await headers()).get("x-pathname") ?? "";
  if (session.must_change_password && !path.startsWith("/password")) {
    redirect("/password");
  }
  return <Shell session={session}>{children}</Shell>;
}
