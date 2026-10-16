import { api } from "@/lib/session";

export default async function BlamePage({
  params,
}: {
  params: Promise<{ owner: string; name: string; path?: string[] }>;
}) {
  const { owner, name, path = [] } = await params;
  const filePath = path.join("/");
  const file = await api(`/v1/repos/${owner}/${name}/file?path=${encodeURIComponent(filePath)}`);
  const blame = await api(`/v1/repos/${owner}/${name}/blame?path=${encodeURIComponent(filePath)}`);
  const text = (file.json as { text?: string }).text ?? "";
  const ranges = ((blame.json as { ranges?: { start_line: number; count: number; origin: string }[] }).ranges ?? []);
  const lines = text.split("\n");
  return (
    <>
      <h1 className="mono">{filePath}</h1>
      <table>
        <tbody>
          {lines.map((line, index) => {
            const lineNo = index + 1;
            const range = ranges.find((item) => lineNo >= item.start_line && lineNo < item.start_line + item.count);
            return (
              <tr key={lineNo}>
                <td className="mono muted">{range ? range.origin.slice(0, 10) : ""}</td>
                <td className="mono muted">{lineNo}</td>
                <td className="mono">{line}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </>
  );
}
