import { getNotes, titleOf } from '../lib';
export async function GET() {
  const notes = await getNotes();
  return Response.json(notes.map(n => ({ id: n.id, title: titleOf(n), tags: n.data.tags, summary: n.data.summary ?? "", text: (n.body ?? '').replace(/[#*`>\[\]]/g, ' ').replace(/\s+/g, ' ') })));
}
