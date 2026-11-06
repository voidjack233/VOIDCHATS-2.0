import type {MessageRecord} from '../storage/types';

const GROUP_TIME_WINDOW_MS = 5 * 60 * 1000;

export function getMessageLayout(messages: MessageRecord[], hasOlder: boolean) {
  const layout = new Map<string, {startsGroup: boolean; showDateSeparator: boolean}>();
  const chronological = [...messages].reverse();

  chronological.forEach((message, index) => {
    const previous = chronological[index - 1];
    const differentDay = previous && new Date(previous.createdAt).toDateString() !== new Date(message.createdAt).toDateString();
    const timeGap = previous ? message.createdAt - previous.createdAt : 0;
    const startsGroup = !previous || previous.senderVoidId !== message.senderVoidId ||
      previous.direction !== message.direction || Boolean(differentDay) || timeGap < 0 || timeGap >= GROUP_TIME_WINDOW_MS;
    layout.set(message.id, {
      startsGroup,
      showDateSeparator: previous ? Boolean(differentDay) : !hasOlder,
    });
  });
  return layout;
}
