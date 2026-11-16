-- Earlier builds could leave replied conversations in Requests.
DELETE FROM requests
WHERE EXISTS (
    SELECT 1 FROM outgoing_operations WHERE conversation_id = requests.conversation_id
) OR EXISTS (
    SELECT 1 FROM messages WHERE conversation_id = requests.conversation_id AND direction = 1
);
