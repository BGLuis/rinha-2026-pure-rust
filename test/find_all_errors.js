const fs = require('fs');

async function run() {
    const data = JSON.parse(fs.readFileSync('./test-data.json', 'utf8'));
    let fps = [];
    let fns = [];
    
    // Process in batches so we don't overwhelm node
    for (const entry of data.entries) {
        const res = await fetch('http://localhost:9999/fraud-score', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(entry.request)
        });
        const body = await res.json();
        
        if (entry.expected_approved === true && body.approved === false) {
            fps.push(entry.request.id);
        } else if (entry.expected_approved === false && body.approved === true) {
            fns.push(entry.request.id);
        }
    }
    
    console.log("False Positives (Expected True, Got False):");
    console.log(JSON.stringify(fps, null, 2));
    
    console.log("False Negatives (Expected False, Got True):");
    console.log(JSON.stringify(fns, null, 2));
}

run().catch(console.error);
